use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use nwall_ipc::{is_image, is_video, CatalogSource};
use super::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WallhavenDetails {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub uploader: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub purity: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub file_type: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub views: Option<u64>,
    #[serde(default)]
    pub favorites: Option<u64>,
    #[serde(default)]
    pub page_url: Option<String>,
    #[serde(default)]
    pub colors: Vec<String>,
}

pub(crate) const WALLHAVEN_DETAIL_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// Cap concurrent Wallhaven `/w/{id}` requests.
pub const WALLHAVEN_DETAIL_HTTP_MAX: usize = 8;

pub(crate) fn wallhaven_warm_gen() -> &'static std::sync::atomic::AtomicU64 {
    static GEN: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    GEN.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

pub(crate) fn wallhaven_detail_mem() -> &'static Mutex<HashMap<String, WallhavenDetails>> {
    static MEM: OnceLock<Mutex<HashMap<String, WallhavenDetails>>> = OnceLock::new();
    MEM.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn wallhaven_detail_inflight(
) -> &'static Mutex<HashMap<String, Vec<mpsc::Sender<Result<WallhavenDetails>>>>> {
    static INFLIGHT: OnceLock<
        Mutex<HashMap<String, Vec<mpsc::Sender<Result<WallhavenDetails>>>>>,
    > = OnceLock::new();
    INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn wallhaven_detail_http_slots() -> &'static (Mutex<usize>, Condvar) {
    static SLOTS: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    SLOTS.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

pub(crate) struct WallhavenDetailHttpSlot;

impl WallhavenDetailHttpSlot {
    pub(crate) fn acquire() -> Self {
        let (mu, cv) = wallhaven_detail_http_slots();
        let mut n = mu.lock().unwrap();
        while *n >= WALLHAVEN_DETAIL_HTTP_MAX {
            n = cv.wait(n).unwrap();
        }
        *n += 1;
        Self
    }
}

impl Drop for WallhavenDetailHttpSlot {
    fn drop(&mut self) {
        let (mu, cv) = wallhaven_detail_http_slots();
        let mut n = mu.lock().unwrap();
        *n = n.saturating_sub(1);
        cv.notify_one();
    }
}

pub(crate) fn wallhaven_mem_get(id: &str) -> Option<WallhavenDetails> {
    wallhaven_detail_mem()
        .lock()
        .ok()
        .and_then(|g| g.get(id).cloned())
}

pub(crate) fn wallhaven_mem_put(id: &str, details: &WallhavenDetails) {
    if let Ok(mut g) = wallhaven_detail_mem().lock() {
        const MAX: usize = 400;
        if g.len() >= MAX {
            // Drop an arbitrary half when the in-process detail cache grows too large.
            let drop_n = g.len() / 2;
            let keys: Vec<String> = g.keys().take(drop_n).cloned().collect();
            for k in keys {
                g.remove(&k);
            }
        }
        g.insert(id.to_string(), details.clone());
    }
}

pub(crate) fn wallhaven_detail_path(id: &str) -> PathBuf {
    let dir = meta_cache_dir()
        .parent()
        .map(|p| p.join("wallhaven"))
        .unwrap_or_else(|| cache_dir().join("wallhaven"));
    let _ = std::fs::create_dir_all(&dir);
    let safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(24)
        .collect();
    dir.join(format!("{safe}.v3.json"))
}

/// Local Wallhaven detail cache only.
pub fn cached_wallhaven_details(id: &str) -> Option<WallhavenDetails> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    if let Some(d) = wallhaven_mem_get(id) {
        return Some(d);
    }
    let path = wallhaven_detail_path(id);
    let meta = path.metadata().ok()?;
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age > WALLHAVEN_DETAIL_TTL {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let d: WallhavenDetails = serde_json::from_str(&text).ok()?;
    wallhaven_mem_put(id, &d);
    Some(d)
}

pub(crate) fn write_wallhaven_detail_cache(id: &str, details: &WallhavenDetails) {
    wallhaven_mem_put(id, details);
    let path = wallhaven_detail_path(id);
    if let Ok(text) = serde_json::to_string(details) {
        let _ = std::fs::write(path, text);
    }
}

pub fn fetch_wallhaven_details(id: &str, api_key: &str) -> Result<WallhavenDetails> {
    let id = id.trim();
    if id.is_empty() {
        return Err(anyhow!("empty wallhaven id"));
    }
    if let Some(d) = cached_wallhaven_details(id) {
        return Ok(d);
    }

    let (tx, rx) = mpsc::channel();
    let mut lead = false;
    {
        let mut inflight = wallhaven_detail_inflight().lock().unwrap();
        if let Some(waiters) = inflight.get_mut(id) {
            waiters.push(tx);
        } else {
            inflight.insert(id.to_string(), vec![tx]);
            lead = true;
        }
    }

    if !lead {
        return rx
            .recv()
            .unwrap_or_else(|_| Err(anyhow!("wallhaven details canceled")));
    }

    let outcome = {
        let _slot = WallhavenDetailHttpSlot::acquire();
        match wallhaven_details_http(id, api_key) {
            Ok(d) => {
                write_wallhaven_detail_cache(id, &d);
                Ok(d)
            }
            Err(e) => {
                if let Some(d) = read_stale_wallhaven_details(id) {
                    wallhaven_mem_put(id, &d);
                    Ok(d)
                } else {
                    Err(e)
                }
            }
        }
    };

    let waiters = wallhaven_detail_inflight()
        .lock()
        .unwrap()
        .remove(id)
        .unwrap_or_default();
    for w in waiters.into_iter().skip(1) {
        let _ = w.send(match &outcome {
            Ok(d) => Ok(d.clone()),
            Err(_) => Err(anyhow!("wallhaven details failed")),
        });
    }
    outcome
}

pub(crate) fn read_stale_wallhaven_details(id: &str) -> Option<WallhavenDetails> {
    let text = std::fs::read_to_string(wallhaven_detail_path(id)).ok()?;
    let d: WallhavenDetails = serde_json::from_str(&text).ok()?;
    wallhaven_mem_put(id, &d);
    Some(d)
}

/// Warm Wallhaven `/w/{id}` tag cache in the background.
pub fn begin_wallhaven_detail_warm(items: &[RemoteItem], api_key: &str) {
    use std::sync::atomic::Ordering;
    let ids: Vec<String> = items
        .iter()
        .filter(|it| it.tags.is_empty() && it.url.contains("wallhaven"))
        .filter_map(|it| it.id.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string))
        .filter(|id| cached_wallhaven_details(id).is_none())
        .collect();
    if ids.is_empty() {
        return;
    }
    let my = wallhaven_warm_gen().fetch_add(1, Ordering::Relaxed) + 1;
    let api_key = api_key.to_string();
    let (tx, rx) = mpsc::channel::<(u64, String)>();
    let rx = std::sync::Arc::new(Mutex::new(rx));
    let workers = WALLHAVEN_DETAIL_HTTP_MAX.min(ids.len()).max(1);
    for _ in 0..workers {
        let rx = std::sync::Arc::clone(&rx);
        let api_key = api_key.clone();
        std::thread::spawn(move || loop {
            let job = rx.lock().unwrap().recv();
            let Ok((gen, id)) = job else {
                break;
            };
            if wallhaven_warm_gen().load(Ordering::Relaxed) != gen {
                continue;
            }
            let _ = fetch_wallhaven_details(&id, &api_key);
        });
    }
    for id in ids {
        let _ = tx.send((my, id));
    }
}

pub(crate) fn wallhaven_details_http(id: &str, api_key: &str) -> Result<WallhavenDetails> {
    let url = format!("https://wallhaven.cc/api/v1/w/{}", urlencoding_lite(id));
    let mut req = listing_agent().get(&url);
    if !api_key.trim().is_empty() {
        req = req.set("X-API-Key", api_key.trim());
    }
    let parsed: WallhavenDetailResponse = req
        .call()
        .with_context(|| format!("wallhaven details {id}"))?
        .into_json()
        .context("wallhaven details json")?;
    Ok(wallhaven_details_from_item(parsed.data))
}

pub(crate) fn wallhaven_details_from_item(w: WallhavenItem) -> WallhavenDetails {
    let tags = w
        .tags
        .iter()
        .map(|t| t.name.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    let (rw, rh) = parse_resolution(&w.resolution);
    let width = w.dimension_x.filter(|n| *n > 0).or(rw);
    let height = w.dimension_y.filter(|n| *n > 0).or(rh);
    let (uploader, avatar) = match w.uploader {
        Some(ref u) => {
            let name = u.username.trim();
            let uploader = if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            };
            (uploader, u.avatar.as_ref().and_then(wallhaven_avatar_url))
        }
        None => (None, None),
    };
    WallhavenDetails {
        tags,
        uploader,
        avatar,
        purity: {
            let p = w.purity.trim();
            if p.is_empty() {
                None
            } else {
                Some(p.to_string())
            }
        },
        category: {
            let c = w.category.trim();
            if c.is_empty() {
                None
            } else {
                Some(c.to_string())
            }
        },
        file_size: w.file_size.filter(|n| *n > 0),
        file_type: {
            let t = w.file_type.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        },
        width,
        height,
        views: w.views,
        favorites: w.favorites,
        page_url: wallhaven_page_url(&w),
        colors: wallhaven_colors(&w.colors),
    }
}

pub(crate) fn wallhaven_page_url(w: &WallhavenItem) -> Option<String> {
    let short = w.short_url.trim();
    if !short.is_empty() {
        return Some(prefer_https(short));
    }
    let page = w.url.trim();
    if !page.is_empty() {
        return Some(prefer_https(page));
    }
    let id = w.id.trim();
    if id.is_empty() {
        None
    } else {
        Some(format!("https://wallhaven.cc/w/{id}"))
    }
}

pub(crate) fn prefer_https(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("http://") {
        format!("https://{rest}")
    } else {
        url.to_string()
    }
}

pub(crate) fn wallhaven_colors(raw: &[String]) -> Vec<String> {
    raw.iter().filter_map(|c| normalize_hex_color(c)).collect()
}

pub(crate) fn normalize_hex_color(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches('#');
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(format!("#{}", h.to_ascii_lowercase()));
    }
    if h.len() == 3 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        let b = h.as_bytes();
        let expanded = format!(
            "#{0}{0}{1}{1}{2}{2}",
            b[0] as char, b[1] as char, b[2] as char
        );
        return Some(expanded.to_ascii_lowercase());
    }
    None
}

pub(crate) fn wallhaven_avatar_url(av: &WallhavenAvatar) -> Option<String> {
    [&av.px32, &av.px128, &av.px200]
        .into_iter()
        .find_map(|u| {
            u.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

#[derive(Deserialize)]
pub(crate) struct WallhavenDetailResponse {
    data: WallhavenItem,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArchiveDetails {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub creator: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub mediatype: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub collection: Option<String>,
    /// Archive.org views (`downloads` in the API).
    #[serde(default, alias = "downloads")]
    pub views: Option<u64>,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub duration_secs: Option<f64>,
    #[serde(default)]
    pub file_type: Option<String>,
    #[serde(default)]
    pub video: bool,
    #[serde(default)]
    pub image: bool,
    /// From metadata `subject` / `subject[]` (topics).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Short label from `licenseurl` (e.g. `CC BY-NC 4.0`).
    #[serde(default)]
    pub license: Option<String>,
}

pub(crate) const ARCHIVE_DETAIL_TTL: Duration = Duration::from_secs(14 * 24 * 3600);

pub(crate) fn archive_detail_path(id: &str) -> PathBuf {
    let dir = cache_dir().join("archive");
    let _ = std::fs::create_dir_all(&dir);
    let safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(80)
        .collect();
    dir.join(format!("{safe}.json"))
}

pub fn cached_archive_details(id: &str) -> Option<ArchiveDetails> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let path = archive_detail_path(id);
    let meta = path.metadata().ok()?;
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age > ARCHIVE_DETAIL_TTL {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn write_archive_detail_cache(id: &str, details: &ArchiveDetails) {
    if let Ok(text) = serde_json::to_string(details) {
        let _ = std::fs::write(archive_detail_path(id), text);
    }
}

pub fn fetch_archive_details(id: &str) -> Result<ArchiveDetails> {
    let id = id.trim();
    if id.is_empty() {
        return Err(anyhow!("empty archive id"));
    }
    if let Some(d) = cached_archive_details(id) {
        return Ok(d);
    }
    match archive_details_http(id) {
        Ok(d) => {
            write_archive_detail_cache(id, &d);
            Ok(d)
        }
        Err(e) => {
            if let Some(d) = std::fs::read_to_string(archive_detail_path(id))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
            {
                Ok(d)
            } else {
                Err(e)
            }
        }
    }
}

pub(crate) fn archive_details_http(id: &str) -> Result<ArchiveDetails> {
    let url = format!("https://archive.org/metadata/{}", urlencoding_lite(id));
    let raw: Value = archive_agent()
        .get(&url)
        .call()
        .with_context(|| format!("archive.org metadata {id}"))?
        .into_json()
        .context("archive.org metadata json")?;
    let mut details = archive_details_from_json(&raw);
    if details.views.unwrap_or(0) == 0 {
        if let Some(n) = archive_views_http(id) {
            details.views = Some(n);
        }
    }
    Ok(details)
}

pub(crate) fn archive_views_http(id: &str) -> Option<u64> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let url = format!(
        "https://archive.org/advancedsearch.php?q=identifier:{}&fl[]=downloads&rows=1&output=json",
        urlencoding_lite(id)
    );
    let raw: Value = archive_agent().get(&url).call().ok()?.into_json().ok()?;
    let doc = raw
        .pointer("/response/docs/0")
        .or_else(|| raw.pointer("/response/docs")?.as_array()?.first())?;
    doc.get("downloads")
        .and_then(|v| v.as_u64())
        .or_else(|| json_stringish(doc.get("downloads")).and_then(|s| s.parse().ok()))
        .filter(|n| *n > 0)
}

pub(crate) fn archive_details_from_json(raw: &Value) -> ArchiveDetails {
    let md = raw.get("metadata").unwrap_or(raw);
    let title = json_stringish(md.get("title"));
    let creator = json_stringish(md.get("creator")).or_else(|| json_stringish(md.get("uploader")));
    let date = json_stringish(md.get("date"))
        .or_else(|| json_stringish(md.get("year")))
        .or_else(|| json_stringish(md.get("publicdate")).map(|s| s.chars().take(10).collect()));
    let mediatype = json_stringish(md.get("mediatype"));
    let description = json_stringish(md.get("description")).and_then(|s| snippet_text(&s, 140));
    let collection = json_stringish(md.get("collection"));
    let tags = archive_subjects_from_value(md.get("subject"));
    let license = json_stringish(md.get("licenseurl"))
        .or_else(|| json_stringish(md.get("license")))
        .and_then(|s| archive_license_label(&s));
    let views = raw
        .get("downloads")
        .and_then(json_u64)
        .or_else(|| raw.pointer("/item/downloads").and_then(json_u64))
        .or_else(|| md.get("downloads").and_then(json_u64));
    let item_size = raw
        .get("item_size")
        .and_then(json_u64)
        .or_else(|| raw.pointer("/item/item_size").and_then(json_u64))
        .or_else(|| md.get("item_size").and_then(json_u64));
    let runtime = json_stringish(md.get("runtime"))
        .and_then(|s| parse_archive_runtime(&s));
    let files = raw
        .get("files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let picked = pick_archive_file_from_json(&files);
    let (file_size, width, height, duration_secs, file_type, video, image) = match &picked {
        Some(f) => {
            let sz = json_stringish(f.get("size")).and_then(|s| s.parse().ok());
            let w = json_stringish(f.get("width")).and_then(|s| s.parse().ok());
            let h = json_stringish(f.get("height")).and_then(|s| s.parse().ok());
            let dur = json_stringish(f.get("length"))
                .and_then(|s| parse_archive_runtime(&s))
                .or(runtime);
            let name = json_stringish(f.get("name")).unwrap_or_default();
            let ext = Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            let video = ext.as_deref().is_some_and(|e| matches!(e, "mp4" | "webm" | "mkv" | "ogv"));
            let image = ext.as_deref().is_some_and(|e| matches!(e, "jpg" | "jpeg" | "png" | "gif" | "webp"));
            (sz, w, h, dur, ext, video, image)
        }
        None => (item_size, None, None, runtime, None, mediatype.as_deref() == Some("movies"), mediatype.as_deref() == Some("image")),
    };
    ArchiveDetails {
        title,
        creator,
        date,
        mediatype,
        description,
        collection,
        views,
        file_size: file_size.or(item_size),
        width,
        height,
        duration_secs,
        file_type,
        video,
        image,
        tags,
        license,
    }
}

pub(crate) fn json_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| json_stringish(Some(v)).and_then(|s| s.parse().ok()))
}

/// `subject` / `subject[]` → deduped topic tags for the preview cloud.
pub(crate) fn archive_subjects_from_value(v: Option<&Value>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    fn push(tag: &str, out: &mut Vec<String>, seen: &mut std::collections::HashSet<String>) {
        let t = tag.trim();
        if t.is_empty() {
            return;
        }
        let key = t.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(t.to_string());
        }
    }
    match v {
        Some(Value::String(s)) => {
            // Rare: comma-separated single string
            if s.contains(',') && !s.contains("://") {
                for part in s.split(',') {
                    push(part, &mut out, &mut seen);
                }
            } else {
                push(s, &mut out, &mut seen);
            }
        }
        Some(Value::Array(arr)) => {
            for x in arr {
                if let Some(s) = json_stringish(Some(x)) {
                    push(&s, &mut out, &mut seen);
                }
            }
        }
        Some(Value::Number(n)) => push(&n.to_string(), &mut out, &mut seen),
        _ => {}
    }
    out
}

/// Compact license label from Archive `licenseurl` (or a bare license string).
pub(crate) fn archive_license_label(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    if let Some(rest) = lower
        .strip_prefix("http://creativecommons.org/")
        .or_else(|| lower.strip_prefix("https://creativecommons.org/"))
    {
        let path = rest.trim_matches('/');
        if let Some(ver) = path.strip_prefix("publicdomain/zero/") {
            return Some(format!("CC0 {}", ver.trim_matches('/')));
        }
        if let Some(ver) = path.strip_prefix("publicdomain/mark/") {
            return Some(format!("Public Domain Mark {}", ver.trim_matches('/')));
        }
        if let Some(rest) = path.strip_prefix("licenses/") {
            let parts: Vec<&str> = rest.trim_matches('/').split('/').filter(|p| !p.is_empty()).collect();
            if let Some(code_raw) = parts.first() {
                let code = code_raw
                    .split('-')
                    .map(|p| p.to_ascii_uppercase())
                    .collect::<Vec<_>>()
                    .join("-");
                return if let Some(ver) = parts.get(1) {
                    Some(format!("CC {code} {ver}"))
                } else {
                    Some(format!("CC {code}"))
                };
            }
        }
    }
    Some(s.to_string())
}

pub(crate) fn parse_archive_runtime(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<f64>() {
        if n.is_finite() && n > 0.0 {
            return Some(n);
        }
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let m: f64 = parts[0].parse().ok()?;
        let sec: f64 = parts[1].parse().ok()?;
        Some(m * 60.0 + sec)
    } else if parts.len() == 3 {
        let h: f64 = parts[0].parse().ok()?;
        let m: f64 = parts[1].parse().ok()?;
        let sec: f64 = parts[2].parse().ok()?;
        Some(h * 3600.0 + m * 60.0 + sec)
    } else {
        None
    }
}

pub(crate) fn json_stringish(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Array(arr)) => arr.iter().find_map(|x| json_stringish(Some(x))),
        _ => None,
    }
}

pub(crate) fn snippet_text(s: &str, max: usize) -> Option<String> {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return None;
    }
    let n = t.chars().count();
    if n <= max {
        Some(t)
    } else {
        Some(t.chars().take(max.saturating_sub(1)).collect::<String>() + "…")
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GitHubDetails {
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// Repository topics (same for every file in the repo).
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub duration_secs: Option<f64>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub fps: Option<f32>,
}

/// Per-repo license + topics (one `/repos/{owner}/{repo}` call, shared by all files).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct GitHubRepoMeta {
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub topics: Vec<String>,
}

pub(crate) const GITHUB_DETAIL_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

pub(crate) fn github_detail_path(key: &str) -> PathBuf {
    let dir = cache_dir().join("github");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("v2-{}.json", hash_url(key)))
}

/// Last-commit author for a file.
pub(crate) fn github_details_from_commits(raw: &Value, repo: &str) -> GitHubDetails {
    let mut details = GitHubDetails {
        repo: Some(repo.to_string()),
        ..Default::default()
    };
    let Some(c) = raw.as_array().and_then(|a| a.first()) else {
        return details;
    };
    let gh_login = c
        .pointer("/author/login")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let api_avatar = c.pointer("/author/avatar_url").and_then(|v| v.as_str());
    let git_name = c
        .pointer("/commit/author/name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(login) = gh_login {
        details.author = Some(login.to_string());
        details.avatar = github_user_avatar(login, api_avatar);
    } else {
        details.author = git_name.map(str::to_string);
        details.avatar = None;
    }
    details.date = c
        .pointer("/commit/author/date")
        .and_then(|v| v.as_str())
        .map(|s| s.chars().take(10).collect());
    details
}

/// Prefer `github.com/{login}.png` so the cache file has a real image extension.
pub(crate) fn github_user_avatar(login: &str, api_avatar: Option<&str>) -> Option<String> {
    let login = login.trim();
    if !login.is_empty()
        && login.len() <= 39
        && login
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Some(format!("https://github.com/{login}.png?size=64"));
    }
    api_avatar
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn fetch_github_details(item: &RemoteItem, api_key: &str) -> Result<GitHubDetails> {
    let repo = item
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains('/'))
        .ok_or_else(|| anyhow!("github item missing repo"))?;
    let path = item
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(Some(item.name.trim()))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("github item missing path"))?;
    let key = format!("{repo}|{path}");
    let cache = github_detail_path(&key);
    let mut details = GitHubDetails {
        repo: Some(repo.to_string()),
        ..Default::default()
    };
    let mut from_cache = false;
    if let Ok(meta) = cache.metadata() {
        if meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age <= GITHUB_DETAIL_TTL)
        {
            if let Ok(text) = std::fs::read_to_string(&cache) {
                if let Ok(d) = serde_json::from_str::<GitHubDetails>(&text) {
                    details = d;
                    from_cache = true;
                }
            }
        }
    }
    if !from_cache {
        let commit_url = format!(
            "https://api.github.com/repos/{repo}/commits?path={}&per_page=1",
            urlencoding_lite(path)
        );
        let token = github_token_or_env(api_key);
        match github_api_get(&commit_url, &token) {
            Ok(resp) => {
                if let Ok(raw) = resp.into_json::<Value>() {
                    details = github_details_from_commits(&raw, repo);
                }
            }
            Err(e) => log::warn!("github commits {repo} {path}: {e}"),
        }
        fill_github_details_repo_meta(&mut details, repo, &token);
    } else if details.license.is_none() || details.tags.is_empty() {
        let token = github_token_or_env(api_key);
        fill_github_details_repo_meta(&mut details, repo, &token);
    }
    let need_probe = item.video
        && !item.url.trim().is_empty()
        && !details
            .duration_secs
            .is_some_and(|d| d.is_finite() && d > 0.0);
    if need_probe {
        let probe = probe_remote_video_stats(&item.url);
        if details.duration_secs.is_none() {
            details.duration_secs = probe
                .duration_secs
                .filter(|n| n.is_finite() && *n > 0.0);
        }
        if details.width.unwrap_or(0) == 0 {
            details.width = probe.width.filter(|n| *n > 0);
        }
        if details.height.unwrap_or(0) == 0 {
            details.height = probe.height.filter(|n| *n > 0);
        }
        if details.fps.unwrap_or(0.0) == 0.0 {
            details.fps = probe.fps.filter(|f| *f > 0.0);
        }
    }
    if let Ok(text) = serde_json::to_string(&details) {
        let _ = std::fs::write(cache, text);
    }
    Ok(details)
}

pub(crate) fn github_repo_meta_path(repo: &str) -> PathBuf {
    let dir = cache_dir().join("github");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("repo-meta-{}.json", hash_url(repo)))
}

pub(crate) fn github_license_path(repo: &str) -> PathBuf {
    let dir = cache_dir().join("github");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("license-{}.json", hash_url(repo)))
}

pub(crate) fn parse_github_topics(raw: &Value) -> Vec<String> {
    raw.get("topics")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn parse_github_license(raw: &Value) -> Option<String> {
    let spdx = raw
        .pointer("/license/spdx_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && *s != "NOASSERTION");
    let name = raw
        .pointer("/license/name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    spdx.or(name).map(str::to_string)
}

pub(crate) fn cached_github_repo_meta(repo: &str) -> Option<GitHubRepoMeta> {
    let path = github_repo_meta_path(repo);
    if let Ok(meta) = path.metadata() {
        let age = meta.modified().ok()?.elapsed().ok()?;
        if age <= GITHUB_DETAIL_TTL {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(m) = serde_json::from_str::<GitHubRepoMeta>(&text) {
                    return Some(m);
                }
            }
        }
    }
    let legacy = github_license_path(repo);
    let meta = legacy.metadata().ok()?;
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age > GITHUB_DETAIL_TTL {
        return None;
    }
    let text = std::fs::read_to_string(legacy).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let license = v.as_str().map(str::to_string).filter(|s| !s.is_empty());
    license.map(|license| GitHubRepoMeta {
        license: Some(license),
        topics: Vec::new(),
    })
}

pub(crate) fn write_github_repo_meta(repo: &str, meta: &GitHubRepoMeta) {
    if let Ok(text) = serde_json::to_string(meta) {
        let _ = std::fs::write(github_repo_meta_path(repo), text);
    }
}

/// One GET `/repos/{owner}/{repo}` for license + topics (cached for the whole pack).
pub(crate) fn fetch_github_repo_meta(repo: &str, token: &str) -> Option<GitHubRepoMeta> {
    let url = format!("https://api.github.com/repos/{repo}");
    let raw: Value = github_api_get(&url, token).ok()?.into_json().ok()?;
    let meta = GitHubRepoMeta {
        license: parse_github_license(&raw),
        topics: parse_github_topics(&raw),
    };
    // Always cache (including empty topics) so we don't re-hit the API every list.
    write_github_repo_meta(repo, &meta);
    Some(meta)
}

pub(crate) fn fill_github_details_repo_meta(details: &mut GitHubDetails, repo: &str, token: &str) {
    if details.license.is_some() && !details.tags.is_empty() {
        return;
    }
    let Some(meta) = cached_github_repo_meta(repo).or_else(|| fetch_github_repo_meta(repo, token))
    else {
        return;
    };
    if details.license.is_none() {
        details.license = meta.license;
    }
    if details.tags.is_empty() && !meta.topics.is_empty() {
        details.tags = meta.topics;
    }
}

/// Stamp cached (or freshly fetched) repo topics onto every item from `repo`.
pub(crate) fn apply_github_repo_topics(items: &mut [RemoteItem], repo: &str, token: &str) {
    let topics = cached_github_repo_meta(repo)
        .or_else(|| fetch_github_repo_meta(repo, token))
        .map(|m| m.topics)
        .filter(|t| !t.is_empty());
    let Some(topics) = topics else {
        return;
    };
    for it in items {
        if it.tags.is_empty() {
            it.tags = topics.clone();
        }
    }
}

