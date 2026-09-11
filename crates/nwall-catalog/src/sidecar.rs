use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Deserialize;

use nwall_ipc::is_video;
use super::*;

pub(crate) fn meta_cache_dir() -> PathBuf {
    let dir = cache_dir().parent().map(|p| p.to_path_buf()).unwrap_or_else(cache_dir);
    let dir = dir.join("meta");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub(crate) fn sidecar_path(media: &Path) -> PathBuf {
    let mut s = media.as_os_str().to_os_string();
    s.push(".nwall.json");
    PathBuf::from(s)
}

pub(crate) fn meta_cache_file(key: &str) -> PathBuf {
    meta_cache_dir().join(format!("{}.json", hash_url(key)))
}

pub(crate) fn path_meta_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Write catalog stats next to the file and under `~/.cache/nwall/meta/`.
/// Empty fields on a later save keep previously stored sidecar values.
pub fn persist_remote_meta(item: &RemoteItem, dest: &Path) {
    let mut stats = MediaStats::from_remote(item);
    if let Some(existing) = load_path_meta(dest) {
        stats.fill_from(&existing);
    }
    write_stats_file(&sidecar_path(dest), &stats);
    write_stats_file(&meta_cache_file(&path_meta_key(dest)), &stats);
    if !item.url.is_empty() {
        write_stats_file(&meta_cache_file(&item.url), &stats);
    }
}

/// Write library sidecar stats. Empty catalog fields keep the previous sidecar;
/// background-music fields always use `stats` (including an explicit clear).
pub fn persist_path_meta(path: &Path, stats: &MediaStats) {
    let mut merged = stats.clone();
    if let Some(existing) = load_path_meta(path) {
        merged.fill_from(&existing);
    }
    merged.bg_music = stats.bg_music.clone();
    merged.bg_music_volume = stats.bg_music_volume;
    merged.bg_music_mute = stats.bg_music_mute;
    write_stats_file(&sidecar_path(path), &merged);
    write_stats_file(&meta_cache_file(&path_meta_key(path)), &merged);
}

pub(crate) fn looks_like_wallhaven_id(s: &str) -> bool {
    let s = s.trim();
    (5..=7).contains(&s.len())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

pub(crate) fn infer_wallhaven_id(path: &Path, stats: &MediaStats) -> Option<String> {
    if let Some(id) = nonempty_opt(&stats.source_id).filter(|s| looks_like_wallhaven_id(s)) {
        return Some(id.to_string());
    }
    if let Some(u) = nonempty_opt(&stats.page_url) {
        for prefix in ["https://whvn.cc/", "https://wallhaven.cc/w/", "http://whvn.cc/"] {
            if let Some(rest) = u.strip_prefix(prefix) {
                let id = rest.trim_matches('/').split(['?', '#']).next().unwrap_or("");
                if looks_like_wallhaven_id(id) {
                    return Some(id.to_string());
                }
            }
        }
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| looks_like_wallhaven_id(s))?;
    Some(stem.to_string())
}

pub(crate) fn stats_looks_wallhaven(stats: &MediaStats) -> bool {
    let source = stats
        .source
        .as_deref()
        .or(stats.credit.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase();
    if source.contains("wallhaven") {
        return true;
    }
    if let Some(u) = nonempty_opt(&stats.page_url) {
        let u = u.to_ascii_lowercase();
        if u.contains("wallhaven") || u.contains("whvn.cc") {
            return true;
        }
    }
    nonempty_opt(&stats.source_id).is_some_and(looks_like_wallhaven_id)
        && (nonempty_opt(&stats.purity).is_some() || nonempty_opt(&stats.category).is_some())
}

pub(crate) fn stats_looks_archive(stats: &MediaStats) -> bool {
    let source = stats
        .source
        .as_deref()
        .or(stats.credit.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase();
    source.contains("archive")
        || nonempty_opt(&stats.page_url).is_some_and(|u| u.contains("archive.org"))
}

/// Refresh incomplete Discover-equivalent fields on a library sidecar (cache/HTTP).
pub fn enrich_library_meta(path: &Path) -> Option<MediaStats> {
    let mut stats = load_path_meta(path).unwrap_or_default();
    let before = serde_json::to_string(&stats).unwrap_or_default();

    if stats_looks_wallhaven(&stats) || infer_wallhaven_id(path, &stats).is_some() {
        if let Some(id) = infer_wallhaven_id(path, &stats) {
            fill_string(&mut stats.source_id, Some(id.clone()));
            if stats.wallhaven_preview_incomplete() {
                if let Ok(d) = fetch_wallhaven_details(&id, "") {
                    stats.apply_wallhaven_details(&d);
                }
            }
            if nonempty_opt(&stats.page_url).is_none() {
                stats.page_url = Some(format!("https://whvn.cc/{id}"));
            }
            if stats.source.is_none() {
                stats.source = Some("Wallhaven".into());
            }
        }
    } else if stats_looks_archive(&stats) {
        let id = nonempty_opt(&stats.source_id)
            .map(str::to_string)
            .or_else(|| {
                nonempty_opt(&stats.page_url).and_then(|u| {
                    u.trim()
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
            });
        if let Some(id) = id {
            if stats.archive_preview_incomplete() {
                if let Ok(d) = fetch_archive_details(&id) {
                    stats.apply_archive_details(&d, Some(&id));
                }
            }
            if nonempty_opt(&stats.page_url).is_none() {
                stats.page_url = Some(format!("https://archive.org/details/{id}"));
            }
            fill_string(&mut stats.source_id, Some(id));
        }
    } else if nonempty_opt(&stats.repo).is_some() {
        if nonempty_opt(&stats.page_url).is_none() {
            if let (Some(repo), Some(path)) = (
                nonempty_opt(&stats.repo),
                nonempty_opt(&stats.source_id),
            ) {
                stats.page_url = Some(format!(
                    "https://github.com/{repo}/blob/HEAD/{}",
                    percent_encode_path(path.trim_start_matches('/'))
                ));
            }
        }
        if stats.github_preview_incomplete() {
            let item = RemoteItem {
                name: path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("wallpaper")
                    .to_string(),
                url: String::new(),
                id: stats.source_id.clone(),
                repo: stats.repo.clone(),
                credit: stats.credit.clone(),
                page_url: stats.page_url.clone(),
                ..Default::default()
            };
            if let Ok(d) = fetch_github_details(&item, "") {
                stats.apply_github_details(&d);
            }
        }
    }

    let after = serde_json::to_string(&stats).unwrap_or_default();
    if after != before && !after.is_empty() {
        persist_path_meta(path, &stats);
    }
    Some(stats)
}

pub fn load_path_meta(path: &Path) -> Option<MediaStats> {
    read_stats_file(&sidecar_path(path))
        .or_else(|| read_stats_file(&meta_cache_file(&path_meta_key(path))))
}

/// Persist or clear looping background music on a wallpaper's sidecar.
pub fn set_bg_music_meta(
    wallpaper: &Path,
    music: Option<&Path>,
    volume: Option<f32>,
    mute: Option<bool>,
) {
    let mut stats = load_path_meta(wallpaper).unwrap_or_default();
    match music {
        Some(p) => {
            stats.bg_music = Some(p.to_path_buf());
            if let Some(v) = volume {
                stats.bg_music_volume = Some(v.clamp(0.0, 1.0));
            } else if stats.bg_music_volume.is_none() {
                stats.bg_music_volume = Some(0.5);
            }
            if let Some(m) = mute {
                stats.bg_music_mute = Some(m);
            } else if stats.bg_music_mute.is_none() {
                stats.bg_music_mute = Some(false);
            }
        }
        None => {
            stats.bg_music = None;
            stats.bg_music_volume = None;
            stats.bg_music_mute = None;
        }
    }
    persist_path_meta(wallpaper, &stats);
}

pub fn set_bg_music_volume_meta(wallpaper: &Path, volume: f32) {
    let mut stats = load_path_meta(wallpaper).unwrap_or_default();
    if stats.bg_music.is_none() {
        return;
    }
    stats.bg_music_volume = Some(volume.clamp(0.0, 1.0));
    persist_path_meta(wallpaper, &stats);
}

pub fn set_bg_music_mute_meta(wallpaper: &Path, mute: bool) {
    let mut stats = load_path_meta(wallpaper).unwrap_or_default();
    if stats.bg_music.is_none() {
        return;
    }
    stats.bg_music_mute = Some(mute);
    persist_path_meta(wallpaper, &stats);
}

pub fn remove_path_meta(path: &Path) {
    let _ = std::fs::remove_file(sidecar_path(path));
    let _ = std::fs::remove_file(meta_cache_file(&path_meta_key(path)));
}

/// Rename a library file and keep its `*.nwall.json` sidecar next to it.
pub fn rename_media_with_sidecar(from: &Path, to: &Path) -> Result<PathBuf> {
    if from == to {
        return Ok(to.to_path_buf());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, to)
        .with_context(|| format!("rename {} → {}", from.display(), to.display()))?;
    let from_side = sidecar_path(from);
    let to_side = sidecar_path(to);
    if from_side.exists() {
        let _ = std::fs::rename(&from_side, &to_side);
    }
    if let Some(stats) = read_stats_file(&to_side) {
        write_stats_file(&meta_cache_file(&path_meta_key(to)), &stats);
    }
    let _ = std::fs::remove_file(meta_cache_file(&path_meta_key(from)));
    Ok(to.to_path_buf())
}

pub(crate) fn write_stats_file(path: &Path, stats: &MediaStats) {
    if let Ok(text) = serde_json::to_string(stats) {
        let _ = std::fs::write(path, text);
    }
}

pub(crate) fn read_stats_file(path: &Path) -> Option<MediaStats> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Local pixels / size / duration. Does not HTTP. Videos use one short ffprobe.
pub fn probe_file_stats(path: &Path) -> MediaStats {
    let mut stats = MediaStats::default();
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > 0 {
            stats.file_size = Some(meta.len());
        }
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext = ext.trim();
        if !ext.is_empty() {
            stats.file_type = Some(ext.to_ascii_lowercase());
        }
    }
    if is_video(path) {
        stats.merge_probe(&probe_video_ffprobe(path));
    } else if let Some((w, h)) = image_header_size(path) {
        stats.width = Some(w);
        stats.height = Some(h);
    }
    stats
}

pub(crate) fn probe_video_ffprobe(path: &Path) -> MediaStats {
    let Some(p) = path.to_str() else {
        return MediaStats::default();
    };
    probe_ffprobe_input(p, false, Duration::from_secs(8))
}

/// ffprobe remote video URL (range request; timed out).
pub fn probe_remote_video_stats(url: &str) -> MediaStats {
    let url = url.trim();
    if url.is_empty() {
        return MediaStats::default();
    }
    let input = resolve_remote_url(url).unwrap_or_else(|_| url.to_string());
    probe_ffprobe_input(&input, true, Duration::from_secs(12))
}

pub(crate) fn probe_ffprobe_input(input: &str, http: bool, timeout: Duration) -> MediaStats {
    let mut cmd = Command::new("ffprobe");
    cmd.args([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height,codec_name,r_frame_rate,duration",
        "-show_entries",
        "format=duration",
        "-of",
        "json",
    ]);
    if http {
        cmd.args(["-user_agent", UA]);
    }
    cmd.arg(input)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return MediaStats::default();
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return MediaStats::default();
                }
                let mut buf = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_end(&mut buf);
                }
                return parse_ffprobe_json(&buf);
            }
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(40));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return MediaStats::default();
            }
        }
    }
}

pub(crate) fn parse_ffprobe_json(bytes: &[u8]) -> MediaStats {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        streams: Vec<ProbeStream>,
        format: Option<ProbeFormat>,
    }
    #[derive(Deserialize, Default)]
    struct ProbeStream {
        width: Option<u32>,
        height: Option<u32>,
        codec_name: Option<String>,
        r_frame_rate: Option<String>,
        duration: Option<String>,
    }
    #[derive(Deserialize, Default)]
    struct ProbeFormat {
        duration: Option<String>,
    }
    let Ok(p) = serde_json::from_slice::<Probe>(bytes) else {
        return MediaStats::default();
    };
    let st = p.streams.into_iter().next().unwrap_or_default();
    let mut stats = MediaStats {
        width: st.width.filter(|n| *n > 0),
        height: st.height.filter(|n| *n > 0),
        file_type: st.codec_name.filter(|s| !s.trim().is_empty()),
        fps: parse_frame_rate(st.r_frame_rate.as_deref()),
        ..Default::default()
    };
    let dur = parse_secs(st.duration.as_deref())
        .or_else(|| parse_secs(p.format.and_then(|f| f.duration).as_deref()));
    stats.duration_secs = dur;
    stats
}

pub(crate) fn parse_secs(s: Option<&str>) -> Option<f64> {
    let s = s?.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("n/a") {
        return None;
    }
    let v: f64 = s.parse().ok()?;
    (v.is_finite() && v > 0.0).then_some(v)
}

pub(crate) fn parse_frame_rate(s: Option<&str>) -> Option<f32> {
    let s = s?.trim();
    if s.is_empty() || s == "0/0" {
        return None;
    }
    let v = if let Some((a, b)) = s.split_once('/') {
        let a: f64 = a.trim().parse().ok()?;
        let b: f64 = b.trim().parse().ok()?;
        if b == 0.0 {
            return None;
        }
        a / b
    } else {
        s.parse().ok()?
    };
    (v.is_finite() && v > 0.0 && v < 1000.0).then_some(v as f32)
}

pub(crate) fn image_header_size(path: &Path) -> Option<(u32, u32)> {
    let mut buf = [0u8; 64];
    let mut f = File::open(path).ok()?;
    let n = f.read(&mut buf).ok()?;
    let data = &buf[..n];
    png_size(data)
        .or_else(|| jpeg_size(path))
        .or_else(|| webp_size(data))
}

pub(crate) fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(data[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

pub(crate) fn jpeg_size(path: &Path) -> Option<(u32, u32)> {
    let f = File::open(path).ok()?;
    let mut buf = Vec::new();
    f.take(128 * 1024).read_to_end(&mut buf).ok()?;
    if buf.len() < 4 || buf[0] != 0xFF || buf[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 9 < buf.len() {
        if buf[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = buf[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        // SOF0–SOF3, SOF5–SOF7, SOF9–SOF11, SOF13–SOF15
        if matches!(
            marker,
            0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE
                | 0xCF
        ) {
            let h = u16::from_be_bytes([buf[i + 5], buf[i + 6]]) as u32;
            let w = u16::from_be_bytes([buf[i + 7], buf[i + 8]]) as u32;
            return (w > 0 && h > 0).then_some((w, h));
        }
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if i + 3 >= buf.len() {
            break;
        }
        let len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
        i += 2 + len;
    }
    None
}

pub(crate) fn webp_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 30 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return None;
    }
    match &data[12..16] {
        b"VP8X" if data.len() >= 30 => {
            let w = 1 + u32::from_le_bytes([data[24], data[25], data[26], 0]);
            let h = 1 + u32::from_le_bytes([data[27], data[28], data[29], 0]);
            (w > 0 && h > 0).then_some((w, h))
        }
        b"VP8 " if data.len() >= 30 => {
            let w = u16::from_le_bytes([data[26], data[27]]) as u32 & 0x3FFF;
            let h = u16::from_le_bytes([data[28], data[29]]) as u32 & 0x3FFF;
            (w > 0 && h > 0).then_some((w, h))
        }
        b"VP8L" if data.len() >= 25 => {
            let b = u32::from_le_bytes(data[21..25].try_into().ok()?);
            let w = (b & 0x3FFF) + 1;
            let h = ((b >> 14) & 0x3FFF) + 1;
            Some((w, h))
        }
        _ => None,
    }
}

