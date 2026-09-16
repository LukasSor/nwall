use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

use super::client::{
    existing_yt_cache, url_cache_get, url_cache_put, yt_err_is_block, yt_http, yt_id_lock, yt_mem,
    YT_ANDROID_UA, YT_DOWNLOAD_TIMEOUT, YT_MAX_FILE_BYTES,
};
use super::ids::{youtube_music_watch_url, yt_video_id};
use super::player::{innertube_audio_url, resolve_yt_music_audio};
use crate::{cached_path, download_part_path, MusicTrack};

/// Best-effort cache fill (HTTP only). Safe to run while preview streams the URL.
pub fn prefetch_yt_music(track: &MusicTrack) {
    let Ok(id) = yt_video_id(track) else {
        return;
    };
    if existing_yt_cache(&id).is_some() {
        return;
    }
    let Some(url) = url_cache_get(&id) else {
        return;
    };
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let lock = yt_id_lock(&id);
    let Ok(_g) = lock.try_lock() else {
        return;
    };
    if existing_yt_cache(&id).is_some() {
        return;
    }
    let _ = download_yt_http(&id, &url);
}

/// Cache audio into `~/.cache/nwall/remote/` via HTTP from the InnerTube stream URL.
pub fn download_yt_music(track: &MusicTrack) -> Result<PathBuf> {
    let id = yt_video_id(track)?;
    if let Some(hit) = existing_yt_cache(&id) {
        return Ok(hit);
    }
    let id_lock = yt_id_lock(&id);
    let _id_guard = id_lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hit) = existing_yt_cache(&id) {
        return Ok(hit);
    }
    let url = resolve_yt_music_audio(track)?;
    if url.starts_with('/') {
        let p = PathBuf::from(&url);
        if p.is_file() {
            return Ok(p);
        }
    }
    match download_yt_http(&id, &url) {
        Ok(p) => Ok(p),
        Err(e) => {
            if let Ok(mut g) = yt_mem().lock() {
                g.urls.remove(&id);
            }
            let retry = innertube_audio_url(&id)?;
            url_cache_put(id.clone(), retry.clone());
            download_yt_http(&id, &retry).map_err(|_| e)
        }
    }
}

fn download_yt_http(id: &str, url: &str) -> Result<PathBuf> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(anyhow!("not an http audio URL"));
    }
    let dest = yt_cache_dest(id, ext_from_audio_url(url));
    let tmp = download_part_path(&dest);
    let _ = std::fs::remove_file(&tmp);
    let resp = yt_http()
        .get(url)
        .set("User-Agent", YT_ANDROID_UA)
        .timeout(YT_DOWNLOAD_TIMEOUT)
        .call()
        .map_err(|e| {
            let s = e.to_string();
            if yt_err_is_block(&s) || s.contains("401") || s.contains("403") {
                anyhow!("YouTube blocked this stream")
            } else {
                anyhow!("GET YouTube audio: {e}")
            }
        })?;
    if let Some(ct) = resp.header("Content-Type") {
        let ct = ct.to_ascii_lowercase();
        if ct.contains("text/html") || ct.contains("application/json") {
            return Err(anyhow!("GET YouTube audio: got {ct} instead of audio"));
        }
    }
    let expected = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0);
    if expected.is_some_and(|n| n >= YT_MAX_FILE_BYTES) {
        return Err(anyhow!("YouTube audio over size cap"));
    }
    let mut file =
        std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    let written = std::io::copy(&mut resp.into_reader(), &mut file)
        .with_context(|| format!("write {}", tmp.display()))?;
    drop(file);
    if written < 1024 {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!("YouTube audio too small ({written} bytes)"));
    }
    if written > YT_MAX_FILE_BYTES {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!("YouTube audio over size cap"));
    }
    if let Some(exp) = expected {
        if written + 64 < exp && written * 2 < exp {
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow!(
                "YouTube audio truncated ({written} of {exp} bytes)"
            ));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        std::fs::copy(&tmp, &dest).with_context(|| format!("copy to {}: {e}", dest.display()))?;
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(dest)
}

fn ext_from_audio_url(url: &str) -> &'static str {
    let u = url.to_ascii_lowercase();
    if u.contains("mime=audio%2fmp4") || u.contains("mime=audio/mp4") || u.contains("itag=140") {
        "m4a"
    } else if u.contains("itag=18") {
        "mp4"
    } else if u.contains("mime=audio%2fwebm")
        || u.contains("mime=audio/webm")
        || u.contains("itag=251")
        || u.contains("itag=250")
        || u.contains("itag=249")
    {
        "webm"
    } else {
        "webm"
    }
}

fn yt_cache_dest(id: &str, ext: &str) -> PathBuf {
    let url = youtube_music_watch_url(id);
    cached_path(&url, "ytmusic").with_extension(ext)
}
