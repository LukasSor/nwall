use std::io::{ErrorKind, Read};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use super::client::{
    avatar_cache_get, search_cache_get, search_cache_key, search_cache_put, wait_yt_slot,
    yt_err_is_block, yt_http, YT_MUSIC_ROWS, YT_SEARCH_TIMEOUT,
};
use super::ids::{
    parse_youtube_video_id, parse_yt_music_query, youtube_music_thumb_url, youtube_music_watch_url,
    YtMusicQuery,
};
use super::parse::{collect_remix_tracks, track_from_yt_json};
use crate::{urlencoding_lite, MusicTrack};

const YT_REMIX_CLIENT: &str = "WEB_REMIX";
const YT_REMIX_VERSION: &str = "1.20240724.01.00";
const YT_SONGS_PARAMS: &str = "EgWKAQIIAWoKEAkQBRAKEAMQBA==";

pub fn yt_dlp_available() -> bool {
    match Command::new("yt-dlp")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

/// Search YouTube Music (InnerTube WEB_REMIX) or resolve a pasted watch/playlist URL.
pub fn fetch_yt_music(query: &str) -> Result<Vec<MusicTrack>> {
    match parse_yt_music_query(query) {
        YtMusicQuery::Search(q) if q.is_empty() => Ok(Vec::new()),
        YtMusicQuery::Search(q) => {
            let key = search_cache_key(&q);
            if let Some(hit) = search_cache_get(&key) {
                return Ok(hit);
            }
            let tracks = innertube_search(&q)?;
            search_cache_put(key, tracks.clone());
            Ok(tracks)
        }
        YtMusicQuery::Url(url) => {
            if let Some(id) = parse_youtube_video_id(&url) {
                return Ok(vec![track_from_watch_id(&id)]);
            }
            if !yt_dlp_available() {
                return Err(anyhow!("paste a video URL or search by name"));
            }
            yt_dlp_listing(&url, true)
        }
    }
}

fn innertube_search(query: &str) -> Result<Vec<MusicTrack>> {
    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": YT_REMIX_CLIENT,
                "clientVersion": YT_REMIX_VERSION,
                "hl": "en",
            }
        },
        "query": query,
        "params": YT_SONGS_PARAMS,
    });
    let resp = yt_http()
        .post("https://music.youtube.com/youtubei/v1/search?prettyPrint=false")
        .set("Content-Type", "application/json")
        .set("Origin", "https://music.youtube.com")
        .set("Referer", "https://music.youtube.com/")
        .timeout(YT_SEARCH_TIMEOUT)
        .send_json(body)
        .map_err(|e| anyhow!("YouTube Music search: {e}"))?;
    let raw: Value = resp
        .into_json()
        .map_err(|e| anyhow!("YouTube Music search json: {e}"))?;
    let mut tracks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    collect_remix_tracks(&raw, &mut tracks, &mut seen);
    Ok(tracks)
}

fn track_from_watch_id(id: &str) -> MusicTrack {
    let mut track = MusicTrack {
        id: id.to_string(),
        title: id.to_string(),
        creator: None,
        duration: None,
        page_url: youtube_music_watch_url(id),
        download_url: Some(youtube_music_watch_url(id)),
        size: None,
        license_url: None,
        thumb_url: Some(youtube_music_thumb_url(id)),
        avatar: None,
    };
    let oembed = format!(
        "https://www.youtube.com/oembed?url={}&format=json",
        urlencoding_lite(&format!("https://www.youtube.com/watch?v={id}"))
    );
    if let Ok(resp) = yt_http()
        .get(&oembed)
        .timeout(Duration::from_secs(8))
        .call()
    {
        if let Ok(v) = resp.into_json::<Value>() {
            if let Some(title) = v.get("title").and_then(|x| x.as_str()) {
                let title = title.trim();
                if !title.is_empty() {
                    track.title = title.to_string();
                }
            }
            if let Some(author) = v.get("author_name").and_then(|x| x.as_str()) {
                let author = author.trim();
                if !author.is_empty() {
                    track.creator = Some(author.to_string());
                }
            }
        }
    }
    if let Some(Some(url)) = avatar_cache_get(id) {
        track.avatar = Some(url);
    }
    track
}

fn yt_dlp_listing(target: &str, playlist: bool) -> Result<Vec<MusicTrack>> {
    let mut args = vec![
        "--ignore-config",
        "--no-warnings",
        "--skip-download",
        "--flat-playlist",
        "--dump-json",
        "--no-check-formats",
        "--socket-timeout",
        "20",
    ];
    let end = YT_MUSIC_ROWS.to_string();
    if playlist {
        args.extend(["--playlist-end", end.as_str()]);
    } else {
        args.push("--no-playlist");
    }
    args.push(target);
    let output = run_yt_dlp(&args, YT_SEARCH_TIMEOUT)?;
    if !output.status.success() {
        return Err(yt_dlp_error(&output, "search"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut tracks = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        let Ok(raw) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(track) = track_from_yt_json(&raw) {
            tracks.push(track);
            if tracks.len() >= YT_MUSIC_ROWS {
                break;
            }
        }
    }
    Ok(tracks)
}

fn run_yt_dlp(args: &[&str], timeout: Duration) -> Result<Output> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    wait_yt_slot();
    let mut child = Command::new("yt-dlp")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                anyhow!("install yt-dlp to use YouTube Music")
            } else {
                anyhow!("could not run yt-dlp: {e}")
            }
        })?;
    let mut stdout_pipe = child.stdout.take().context("yt-dlp stdout")?;
    let mut stderr_pipe = child.stderr.take().context("yt-dlp stderr")?;
    let out_h = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let err_h = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(anyhow!("YouTube Music timed out"));
            }
            Ok(None) => thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(anyhow!("yt-dlp: {e}")),
        }
    };
    let stdout = out_h.join().unwrap_or_default();
    let stderr = err_h.join().unwrap_or_default();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn yt_dlp_error(output: &Output, what: &str) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let msg = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .last()
        .unwrap_or("");
    if yt_err_is_block(msg) || yt_err_is_block(&stderr) {
        anyhow!("YouTube blocked this stream")
    } else if msg.is_empty() {
        anyhow!("yt-dlp {what} exited with {}", output.status)
    } else {
        anyhow!("{msg}")
    }
}
