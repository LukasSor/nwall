use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use nwall_ipc::{cache_dir, hash_url, CatalogSource};
use super::types::UA;
use super::*;

/// Discover / slideshow search parameters. Defaults stay SFW.
#[derive(Clone, Debug)]
pub struct SearchOpts {
    pub query: String,
    /// 1-based page index.
    pub page: u32,
    pub purity_sfw: bool,
    pub purity_sketchy: bool,
    pub purity_nsfw: bool,
    pub cat_general: bool,
    pub cat_anime: bool,
    pub cat_people: bool,
    /// Wallhaven: `favorites` | `date_added` | `relevance` | `random` | `views` | `toplist`.
    pub sorting: String,
    /// Wallhaven `topRange` when sorting is toplist: `1d`/`3d`/`1w`/`1M`/`3M`/`6M`/`1y`.
    pub top_range: String,
    /// Wallhaven minimum resolution, e.g. `1920x1080`. Empty = any.
    pub atleast: String,
    /// Wallhaven aspect ratio, e.g. `16x9`. Empty = any.
    pub ratios: String,
    /// Wallhaven: hide AI-tagged art (`ai_art_filter=1`).
    pub hide_ai: bool,
    /// Pixabay safe-search.
    pub safesearch: bool,
    pub order: String,
    /// Pixabay video category (empty = all).
    pub category: String,
    /// Pixabay `video_type`: `all` | `film` | `animation`.
    pub video_type: String,
    /// Wallhaven `sorting=random` seed. Empty = generate one (always page 1).
    pub seed: String,
    /// GitHub / Archive / Pixabay: `all` | `image` | `video`.
    pub media: String,
    /// GitHub Discover: only include these `owner/repo` names. Empty = all repos in the source.
    pub github_repos: Vec<String>,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self {
            query: String::new(),
            page: 1,
            purity_sfw: true,
            purity_sketchy: false,
            purity_nsfw: false,
            cat_general: true,
            cat_anime: true,
            cat_people: true,
            sorting: "favorites".into(),
            top_range: "1M".into(),
            atleast: String::new(),
            ratios: String::new(),
            hide_ai: false,
            safesearch: true,
            order: "popular".into(),
            category: String::new(),
            video_type: "all".into(),
            seed: String::new(),
            media: "all".into(),
            github_repos: Vec::new(),
        }
    }
}

impl SearchOpts {
    pub(crate) fn page(&self) -> u32 {
        self.page.max(1)
    }

    pub(crate) fn wallhaven_categories(&self) -> String {
        let bits = format!(
            "{}{}{}",
            u8::from(self.cat_general),
            u8::from(self.cat_anime),
            u8::from(self.cat_people)
        );
        if bits == "000" {
            "111".into()
        } else {
            bits
        }
    }

    pub(crate) fn wallhaven_purity(&self) -> String {
        let bits = format!(
            "{}{}{}",
            u8::from(self.purity_sfw),
            u8::from(self.purity_sketchy),
            u8::from(self.purity_nsfw)
        );
        if bits == "000" {
            "100".into()
        } else {
            bits
        }
    }

    pub(crate) fn needs_wallhaven_key(&self) -> bool {
        self.purity_sketchy || self.purity_nsfw
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FetchResult {
    pub items: Vec<RemoteItem>,
    pub page: u32,
    #[serde(default)]
    pub last_page: Option<u32>,
}

impl FetchResult {
    pub(crate) fn unpaged(items: Vec<RemoteItem>) -> Self {
        Self {
            items,
            page: 1,
            last_page: Some(1),
        }
    }
}

pub fn supports_paging(kind: &str) -> bool {
    matches!(kind, "wallhaven" | "pixabay" | "coverr" | "archive" | "github")
}

pub(crate) const GITHUB_PAGE_SIZE: u32 = 36;

pub(crate) const DOWNLOAD_HTTP_TIMEOUT_SECS: u64 = 60;
pub(crate) const LISTING_HTTP_TIMEOUT_SECS: u64 = 12;
pub(crate) const GITHUB_HTTP_TIMEOUT_SECS: u64 = 45;
pub(crate) const GITHUB_LIST_CONCURRENCY: usize = 4;
pub const REMOTE_THUMB_HTTP_MAX: usize = 8;

pub(crate) fn agent() -> ureq::Agent {
    // Shared agent so Discover thumbs + downloads reuse TLS / keep-alive.
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::AgentBuilder::new()
                .user_agent(UA)
                .timeout(std::time::Duration::from_secs(DOWNLOAD_HTTP_TIMEOUT_SECS))
                .max_idle_connections(32)
                .max_idle_connections_per_host(8)
                .build()
        })
        .clone()
}

pub(crate) fn listing_agent() -> ureq::Agent {
    // Reuse one agent so listing + `/w/{id}` warm/enrich keep TLS + keep-alive.
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::AgentBuilder::new()
                .user_agent(UA)
                .timeout(std::time::Duration::from_secs(LISTING_HTTP_TIMEOUT_SECS))
                .max_idle_connections(32)
                .max_idle_connections_per_host(10)
                .build()
        })
        .clone()
}

pub(crate) fn github_agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::AgentBuilder::new()
                .user_agent(UA)
                .timeout(std::time::Duration::from_secs(GITHUB_HTTP_TIMEOUT_SECS))
                .max_idle_connections(16)
                .max_idle_connections_per_host(8)
                .build()
        })
        .clone()
}

pub(crate) fn ext_from_url(url: &str) -> &str {
    let path = url.split('?').next().unwrap_or(url);
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
}

pub(crate) fn cache_ext(url: &str, hint: &str) -> String {
    let ext = ext_from_url(url).to_ascii_lowercase();
    if hint == "avatar" && !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") {
        "png".into()
    } else {
        ext
    }
}

/// Cache path for a remote URL (same layout `download` uses).
pub fn cached_path(url: &str, hint: &str) -> PathBuf {
    let ext = cache_ext(url, hint);
    let stem: String = hint
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(40)
        .collect();
    cache_dir().join(format!("{}-{stem}.{ext}", hash_url(url)))
}

/// Ceiling for `~/.cache/nwall/remote` (Discover thumbs and downloads).
const REMOTE_CACHE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const THUMBS_CACHE_MAX_BYTES: u64 = 256 * 1024 * 1024;

fn prune_dir(dir: &Path, max_bytes: u64) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let used = meta
            .accessed()
            .or_else(|_| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        total += meta.len();
        files.push((used, meta.len(), entry.path()));
    }
    if total <= max_bytes {
        return 0;
    }

    files.sort_by_key(|(used, _, _)| *used);
    let mut freed = 0u64;
    for (_, len, path) in files {
        if total.saturating_sub(freed) <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            freed += len;
        }
    }
    freed
}

pub fn prune_disk_caches() {
    let remote = prune_dir(&cache_dir(), REMOTE_CACHE_MAX_BYTES);
    let thumbs = prune_dir(&thumbs_dir(), THUMBS_CACHE_MAX_BYTES);
    if remote + thumbs > 0 {
        log::info!(
            "cache prune: freed {} MB remote, {} MB thumbs",
            remote / (1024 * 1024),
            thumbs / (1024 * 1024)
        );
    }
}

pub fn thumbs_dir() -> PathBuf {
    let mut dir = cache_dir();
    dir.pop();
    dir.push("thumbs");
    dir
}

pub fn matches_query(name: &str, query: &str) -> bool {
    let tokens: Vec<String> = query
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if tokens.is_empty() {
        return true;
    }
    let n = name.to_ascii_lowercase();
    tokens.iter().any(|t| n.contains(t))
}

pub(crate) fn filter_by_query(items: Vec<RemoteItem>, query: &str) -> Vec<RemoteItem> {
    if query.trim().is_empty() {
        return items;
    }
    items
        .into_iter()
        .filter(|it| matches_query(&it.name, query))
        .collect()
}

/// `all` (default) | `image`/`images` | `video`/`videos`. Gif is video via `is_video`.
pub(crate) fn filter_by_media(items: Vec<RemoteItem>, media: &str) -> Vec<RemoteItem> {
    match media.trim().to_ascii_lowercase().as_str() {
        "image" | "images" => items.into_iter().filter(|it| !it.video).collect(),
        "video" | "videos" => items.into_iter().filter(|it| it.video).collect(),
        _ => items,
    }
}

/// Empty `repos` = no filter. Otherwise keep items whose `repo` matches.
pub(crate) fn filter_by_github_repos(items: Vec<RemoteItem>, repos: &[String]) -> Vec<RemoteItem> {
    let selected: Vec<String> = repos
        .iter()
        .map(|r| r.trim().to_ascii_lowercase())
        .filter(|r| !r.is_empty())
        .collect();
    if selected.is_empty() {
        return items;
    }
    items
        .into_iter()
        .filter(|it| {
            it.repo
                .as_deref()
                .map(|r| selected.iter().any(|s| s.eq_ignore_ascii_case(r)))
                .unwrap_or(false)
        })
        .collect()
}

/// Unique `owner/repo` names in configured order (for Discover repo filter UI).
pub fn github_unique_repos(src: &CatalogSource) -> Vec<String> {
    let mut out = Vec::new();
    for (repo, _) in src.github_targets() {
        if !out.iter().any(|r| r == &repo) {
            out.push(repo);
        }
    }
    out
}

pub fn download(url: &str, hint: &str) -> Result<PathBuf> {
    let url = resolve_download_url(url)?;
    let dest = cached_path(&url, hint);
    if dest.exists() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(dest);
    }
    download_to_dest(&url, &dest)
}

/// Force a fresh GET into the cache path.
pub fn redownload(url: &str, hint: &str) -> Result<PathBuf> {
    let url = resolve_download_url(url)?;
    let dest = cached_path(&url, hint);
    let _ = std::fs::remove_file(&dest);
    download_to_dest(&url, &dest)
}

pub(crate) fn download_part_path(dest: &Path) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let name = dest
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    dest.with_file_name(format!(
        "{name}.part-{}",
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

pub(crate) fn download_to_dest(url: &str, dest: &Path) -> Result<PathBuf> {
    let tmp = download_part_path(dest);
    let _ = std::fs::remove_file(&tmp);
    let resp = match agent().get(url).call() {
        Ok(r) => r,
        Err(e) => {
            log::warn!("download failed {url}: {e}");
            return Err(anyhow!("GET {url}: {e}"));
        }
    };
    if let Some(ct) = resp.header("Content-Type") {
        let ct = ct.to_ascii_lowercase();
        if ct.contains("text/html") || ct.contains("application/json") {
            return Err(anyhow!("GET {url}: got {ct} instead of media"));
        }
    }
    let expected = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0);
    let mut file = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    let written = match std::io::copy(&mut resp.into_reader(), &mut file) {
        Ok(n) => n,
        Err(e) => {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            log::warn!("download write failed {url} -> {}: {e}", tmp.display());
            return Err(e).with_context(|| format!("write {}", tmp.display()));
        }
    };
    drop(file);
    if written == 0 {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!("GET {url}: empty body"));
    }
    if let Some(exp) = expected {
        if written != exp {
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow!(
                "GET {url}: truncated ({written} of {exp} bytes)"
            ));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, dest) {
        match std::fs::copy(&tmp, dest) {
            Ok(_) => {
                let _ = std::fs::remove_file(&tmp);
            }
            Err(e2) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e2).with_context(|| {
                    format!("rename/copy {} -> {} (rename: {e})", tmp.display(), dest.display())
                });
            }
        }
    }
    Ok(dest.to_path_buf())
}

pub fn download_thumb(url: &str) -> Result<PathBuf> {
    let dest = cached_path(url, "thumb");
    if dest.exists() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(dest);
    }
    let _slot = RemoteThumbHttpSlot::acquire();
    download(url, "thumb")
}

/// Like `download_thumb`, but always fetches again.
pub fn redownload_thumb(url: &str) -> Result<PathBuf> {
    let _slot = RemoteThumbHttpSlot::acquire();
    redownload(url, "thumb")
}

pub(crate) fn remote_thumb_http_slots() -> &'static (Mutex<usize>, Condvar) {
    static SLOTS: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    SLOTS.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

pub(crate) struct RemoteThumbHttpSlot;

impl RemoteThumbHttpSlot {
    pub(crate) fn acquire() -> Self {
        let (mu, cv) = remote_thumb_http_slots();
        let mut n = mu.lock().unwrap();
        while *n >= REMOTE_THUMB_HTTP_MAX {
            n = cv.wait(n).unwrap();
        }
        *n += 1;
        Self
    }
}

impl Drop for RemoteThumbHttpSlot {
    fn drop(&mut self) {
        let (mu, cv) = remote_thumb_http_slots();
        let mut n = mu.lock().unwrap();
        *n = n.saturating_sub(1);
        cv.notify_one();
    }
}

/// Download into `dir` using a readable filename. Returns the final path.
pub fn download_to_dir(url: &str, dir: &Path, hint: &str) -> Result<PathBuf> {
    download_to_library(url, dir, hint, None)
}

/// Like `download_to_dir`, but `unique` (source id) avoids tag-name collisions.
pub fn download_to_library(
    url: &str,
    dir: &Path,
    hint: &str,
    unique: Option<&str>,
) -> Result<PathBuf> {
    let url = resolve_download_url(url)?;
    std::fs::create_dir_all(dir)?;
    let ext = ext_from_url(&url);
    let hint_stem = Path::new(hint)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(hint);
    let mut stem = sanitize_file_stem(hint_stem);
    if stem.is_empty() {
        stem = unique
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(sanitize_file_stem)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| hash_url(&url));
    }
    let dest = pick_library_dest(dir, &stem, ext, unique);
    if dest.exists() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(dest);
    }
    let cached = download(&url, hint)?;
    if cached != dest {
        std::fs::copy(&cached, &dest)
            .with_context(|| format!("copy to {}", dest.display()))?;
    }
    Ok(dest)
}

pub(crate) fn pick_library_dest(dir: &Path, stem: &str, ext: &str, unique: Option<&str>) -> PathBuf {
    let primary = dir.join(format!("{stem}.{ext}"));
    if !primary.exists() {
        return primary;
    }
    let Some(id) = unique.map(str::trim).filter(|s| !s.is_empty()) else {
        return primary;
    };
    if let Some(meta) = load_path_meta(&primary) {
        if meta.source_id.as_deref() == Some(id) {
            return primary;
        }
    } else if primary.file_stem().and_then(|s| s.to_str()) == Some(id) {
        return primary;
    }
    let extra = sanitize_file_stem(id);
    if extra.is_empty() || extra == stem {
        return primary;
    }
    dir.join(format!("{stem}-{extra}.{ext}"))
}

/// Expand Archive.org item URLs (`…/download/{id}`) to a concrete file URL.
pub(crate) fn resolve_download_url(url: &str) -> Result<String> {
    if let Some(id) = archive_unresolved_identifier(url) {
        let http = archive_agent();
        return archive_pick_file_url(&http, id).ok_or_else(|| {
            anyhow!("archive.org: no playable image/video under the size cap for '{id}'")
        });
    }
    Ok(url.to_string())
}

/// `https://archive.org/download/{id}` or `…/{id}/` (no filename yet).
pub(crate) fn archive_unresolved_identifier(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://archive.org/download/")
        .or_else(|| url.strip_prefix("http://archive.org/download/"))?;
    let id = rest.trim_matches('/');
    if id.is_empty() || id.contains('/') {
        return None;
    }
    Some(id)
}

pub fn needs_api_key(src: &CatalogSource) -> bool {
    matches!(src.kind.as_str(), "pixabay" | "coverr")
}

pub fn shows_api_key_entry(src: &CatalogSource) -> bool {
    matches!(
        src.kind.as_str(),
        "pixabay" | "coverr" | "wallhaven" | "nasa" | "github"
    )
}

pub fn has_api_key(src: &CatalogSource) -> bool {
    if !src.api_key.trim().is_empty() {
        return true;
    }
    match src.kind.as_str() {
        "pixabay" => env_key("PIXABAY_API_KEY"),
        "coverr" => env_key("COVERR_API_KEY"),
        "wallhaven" => env_key("WALLHAVEN_API_KEY"),
        "github" => env_key("GITHUB_TOKEN") || env_key("GH_TOKEN"),
        _ => true,
    }
}

pub fn is_selectable(src: &CatalogSource) -> bool {
    !needs_api_key(src) || has_api_key(src)
}

pub fn selectable_sources(sources: &[CatalogSource]) -> Vec<CatalogSource> {
    sources
        .iter()
        .filter(|s| is_selectable(s))
        .cloned()
        .collect()
}

pub(crate) fn env_key(name: &str) -> bool {
    std::env::var(name).map(|s| !s.trim().is_empty()).unwrap_or(false)
}

pub fn missing_key_help(src: &CatalogSource) -> String {
    match src.kind.as_str() {
        "pixabay" => {
            "Pixabay needs an API key.\n\nAdd it next to Pixabay in Settings.\nGet a free key at pixabay.com/api/docs/".into()
        }
        "coverr" => {
            "Coverr needs an API key.\n\nAdd it next to Coverr in Settings.\nFree key at coverr.co/developers\n(attribution: Video from Coverr)".into()
        }
        "wallhaven" => {
            "Wallhaven NSFW/sketchy needs an API key — add it next to Wallhaven in Settings (wallhaven.cc/settings/account), enable NSFW on the key.".into()
        }
        "nasa" => {
            "NASA APOD works without a key but is rate-limited.\n\nAdd a free key next to NASA APOD in Settings (api.nasa.gov).".into()
        }
        "github" => {
            "GitHub listing works without a token (60 API requests/hour) but the built-in pack can exhaust that quickly.\n\nAdd a fine-grained or classic PAT next to GitHub in Settings, or set GITHUB_TOKEN / GH_TOKEN (github.com/settings/tokens — public repo read is enough).".into()
        }
        _ => format!("{} needs an API key — add it next to the name in Settings.", src.name),
    }
}

pub fn source_api_key(src: &CatalogSource) -> String {
    if !src.api_key.trim().is_empty() {
        return src.api_key.trim().to_string();
    }
    match src.kind.as_str() {
        "pixabay" => std::env::var("PIXABAY_API_KEY").unwrap_or_default(),
        "coverr" => std::env::var("COVERR_API_KEY").unwrap_or_default(),
        "wallhaven" => std::env::var("WALLHAVEN_API_KEY").unwrap_or_default(),
        "nasa" => std::env::var("NASA_API_KEY").unwrap_or_default(),
        "github" => github_token_env(),
        _ => String::new(),
    }
}

pub(crate) fn github_token_or_env(api_key: &str) -> String {
    let t = api_key.trim();
    if !t.is_empty() {
        return t.to_string();
    }
    github_token_env()
}

pub(crate) fn github_token_env() -> String {
    for name in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(name) {
            let t = v.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    String::new()
}

