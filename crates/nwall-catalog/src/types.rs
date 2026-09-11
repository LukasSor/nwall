use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::*;

pub const UA: &str = "nwall/0.1 (niri wallpaper)";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RemoteItem {
    pub name: String,
    pub video: bool,
    pub url: String,
    pub thumb: Option<String>,
    #[serde(default)]
    pub credit: Option<String>,
    /// Source id for downloads/tooltips.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub purity: Option<String>,
    #[serde(default)]
    pub uploader: Option<String>,
    /// Wallhaven profile picture URL (32px or 128px).
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub collection: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub downloads: Option<u64>,
    #[serde(default)]
    pub views: Option<u64>,
    #[serde(default)]
    pub favorites: Option<u64>,
    /// Pixabay comment count (and similar sources).
    #[serde(default)]
    pub comments: Option<u64>,
    /// Wallhaven page (`short_url` or `url`), not the image file.
    #[serde(default)]
    pub page_url: Option<String>,
    /// Wallhaven palette hex colors (usually ~5).
    #[serde(default)]
    pub colors: Vec<String>,
    /// GitHub `owner/repo`.
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub duration_secs: Option<f64>,
    #[serde(default)]
    pub file_type: Option<String>,
    #[serde(default)]
    pub fps: Option<f32>,
}

impl RemoteItem {
    /// Tile caption under the thumb.
    pub fn tile_label(&self) -> String {
        if !prefers_name_over_tags(self) {
            if let Some(tags) = tag_caption(&self.tags, 3) {
                return tags;
            }
        }
        let name = self.name.trim();
        if !name.is_empty() && !looks_like_resolution_caption(name) {
            return name.to_string();
        }
        if let Some(id) = self.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return format!("#{id}");
        }
        if !name.is_empty() {
            return name.to_string();
        }
        String::from("Wallpaper")
    }

    pub fn apply_wallhaven_details(&mut self, d: &WallhavenDetails) {
        if !d.tags.is_empty() {
            self.tags = d.tags.clone();
        }
        fill_string(&mut self.uploader, d.uploader.clone());
        fill_string(&mut self.avatar, d.avatar.clone());
        fill_string(&mut self.purity, d.purity.clone());
        fill_string(&mut self.category, d.category.clone());
        fill_string(&mut self.file_type, d.file_type.clone());
        if self.file_size.unwrap_or(0) == 0 {
            self.file_size = d.file_size.filter(|n| *n > 0);
        }
        if self.width.unwrap_or(0) == 0 {
            self.width = d.width.filter(|n| *n > 0);
        }
        if self.height.unwrap_or(0) == 0 {
            self.height = d.height.filter(|n| *n > 0);
        }
        if d.views.is_some() {
            self.views = d.views;
        }
        if d.favorites.is_some() {
            self.favorites = d.favorites;
        }
        fill_string(&mut self.page_url, d.page_url.clone());
        if self.colors.is_empty() && !d.colors.is_empty() {
            self.colors = d.colors.clone();
        }
    }

    pub fn apply_archive_details(&mut self, d: &ArchiveDetails) {
        if let Some(t) = nonempty_opt(&d.title) {
            if self.name.trim().is_empty()
                || looks_like_opaque_stem(self.name.trim())
                || self.name.trim() == self.id.as_deref().unwrap_or("")
            {
                self.name = t.to_string();
            }
        }
        fill_string(&mut self.uploader, d.creator.clone());
        fill_string(&mut self.category, d.mediatype.clone());
        fill_string(&mut self.file_type, d.file_type.clone());
        fill_string(&mut self.date, d.date.clone());
        fill_string(&mut self.collection, d.collection.clone());
        fill_string(&mut self.description, d.description.clone());
        fill_string(&mut self.license, d.license.clone());
        if !d.tags.is_empty() {
            self.tags = d.tags.clone();
        }
        if self.views.unwrap_or(0) == 0 {
            self.views = d.views.filter(|n| *n > 0).or_else(|| {
                self.downloads.filter(|n| *n > 0)
            });
        }
        if self.views.unwrap_or(0) > 0 {
            self.downloads = None;
        }
        if self.file_size.unwrap_or(0) == 0 {
            self.file_size = d.file_size.filter(|n| *n > 0);
        }
        if self.width.unwrap_or(0) == 0 {
            self.width = d.width.filter(|n| *n > 0);
        }
        if self.height.unwrap_or(0) == 0 {
            self.height = d.height.filter(|n| *n > 0);
        }
        if self.duration_secs.is_none() {
            self.duration_secs = d.duration_secs.filter(|n| n.is_finite() && *n > 0.0);
        }
        if d.video {
            self.video = true;
        } else if d.image {
            self.video = false;
        }
        ensure_archive_page_url(self);
    }

    pub fn apply_github_details(&mut self, d: &GitHubDetails) {
        if self.tags.is_empty() && !d.tags.is_empty() {
            self.tags = d.tags.clone();
        }
        fill_string(&mut self.uploader, d.author.clone());
        fill_string(&mut self.avatar, d.avatar.clone());
        fill_string(&mut self.date, d.date.clone());
        fill_string(&mut self.license, d.license.clone());
        fill_string(&mut self.repo, d.repo.clone());
        if self.width.unwrap_or(0) == 0 {
            self.width = d.width.filter(|n| *n > 0);
        }
        if self.height.unwrap_or(0) == 0 {
            self.height = d.height.filter(|n| *n > 0);
        }
        if self.duration_secs.is_none() {
            self.duration_secs = d.duration_secs.filter(|n| n.is_finite() && *n > 0.0);
        }
        if self.fps.unwrap_or(0.0) == 0.0 {
            self.fps = d.fps.filter(|f| *f > 0.0);
        }
        ensure_github_page_url(self);
    }

    pub fn stats(&self) -> MediaStats {
        MediaStats::from_remote(self)
    }

    /// `1920×1080 · General` for tooltips / preview meta (not the tile caption).
    pub fn resolution_meta(&self) -> String {
        let mut parts = Vec::new();
        match (self.width, self.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => parts.push(format!("{w}×{h}")),
            _ => {}
        }
        if let Some(c) = self
            .category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            parts.push(title_case_ascii(c));
        }
        parts.join(" · ")
    }

    /// Hover text: `1920×1080 · General · #id` plus credit.
    pub fn tooltip_label(&self) -> String {
        let mut parts = Vec::new();
        let res = self.resolution_meta();
        if !res.is_empty() {
            parts.push(res);
        }
        if let Some(id) = self.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            parts.push(format!("#{id}"));
        }
        let mut tip = parts.join(" · ");
        if let Some(c) = self.credit.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if tip.is_empty() {
                tip = c.to_string();
            } else {
                tip = format!("{tip}\n{c}");
            }
        }
        if tip.is_empty() {
            self.tile_label()
        } else {
            tip
        }
    }

    /// Stable cache stem (prefer source id over display name).
    pub fn file_hint(&self) -> String {
        if let Some(id) = self.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return id.to_string();
        }
        self.name.clone()
    }

    /// Library filename stem: tags when known, else a real title, else the source id.
    pub fn library_file_stem(&self) -> String {
        library_download_stem(self)
    }
}

pub(crate) fn fill_string(slot: &mut Option<String>, val: Option<String>) {
    let blank = slot
        .as_deref()
        .map(|s| s.trim().is_empty())
        .unwrap_or(true);
    if blank {
        if let Some(v) = val {
            if !v.trim().is_empty() {
                *slot = Some(v);
            }
        }
    }
}

/// First `n` non-empty tag names joined with ` · `.
pub fn tag_caption(tags: &[String], n: usize) -> Option<String> {
    let names: Vec<&str> = tags
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .take(n)
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(" · "))
    }
}

/// Filesystem-safe stem from the first `n` tags (`matterhorn-alps-snow`).
pub fn tag_file_stem(tags: &[String], n: usize) -> Option<String> {
    let parts: Vec<String> = tags
        .iter()
        .map(|t| sanitize_file_stem(t.trim()))
        .filter(|t| !t.is_empty())
        .take(n)
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("-"))
    }
}

/// Replace path separators and other unsafe filename characters with `-`.
pub fn sanitize_file_stem(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        let replace = c.is_control()
            || c.is_whitespace()
            || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '.' | ',' | '·');
        if replace {
            if !out.is_empty() && !prev_dash {
                out.push('-');
                prev_dash = true;
            }
            continue;
        }
        out.push(c);
        prev_dash = c == '-';
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.chars().count() > 80 {
        out = out.chars().take(80).collect();
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

pub(crate) fn prefers_name_over_tags(item: &RemoteItem) -> bool {
    item.repo
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty())
        || is_archive_remote(item)
}

pub(crate) fn is_archive_remote(item: &RemoteItem) -> bool {
    item.url.to_ascii_lowercase().contains("archive.org")
        || item
            .credit
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case("Internet Archive"))
}

pub(crate) fn prefers_title_over_tags_meta(m: &MediaStats) -> bool {
    m.repo
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty())
        || is_archive_meta(m)
}

pub(crate) fn is_archive_meta(m: &MediaStats) -> bool {
    m.source.as_deref() == Some("Archive.org")
        || m.credit
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case("Internet Archive"))
        || m.page_url
            .as_deref()
            .is_some_and(|u| u.to_ascii_lowercase().contains("archive.org"))
}

pub(crate) fn library_download_stem(item: &RemoteItem) -> String {
    if !prefers_name_over_tags(item) {
        if let Some(s) = tag_file_stem(&item.tags, 3) {
            return s;
        }
    }
    let has_id = item
        .id
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    if !has_id {
        let raw = item.name.trim();
        let stem = Path::new(raw)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(raw);
        let s = sanitize_file_stem(stem);
        if !s.is_empty() {
            return s;
        }
    }
    let hint = item.file_hint();
    let hint_stem = Path::new(&hint)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&hint);
    let s = sanitize_file_stem(hint_stem);
    if s.is_empty() {
        hint
    } else {
        s
    }
}

pub(crate) fn remote_display_title(item: &RemoteItem) -> Option<String> {
    let t = item.tile_label();
    let t = t.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("wallpaper") {
        return None;
    }
    if t.starts_with('#') && looks_like_opaque_stem(t.trim_start_matches('#')) {
        return None;
    }
    if looks_like_resolution_caption(t) || looks_like_opaque_stem(t) {
        return None;
    }
    Some(t.to_string())
}

/// Gallery / preview heading for a local file (tags, stored title, or filename).
pub fn library_caption(path: &Path) -> String {
    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Wallpaper");
    display_caption(load_path_meta(path).as_ref(), fname)
}

/// Library tile caption.
pub fn display_caption(meta: Option<&MediaStats>, file_name: &str) -> String {
    if let Some(m) = meta {
        let skip_tags = prefers_title_over_tags_meta(m);
        if !skip_tags {
            if let Some(t) = tag_caption(&m.tags, 3) {
                return t;
            }
        }
        if let Some(title) = nonempty_opt(&m.title) {
            if !looks_like_opaque_stem(title) && !looks_like_resolution_caption(title) {
                return title.to_string();
            }
        }
        // Archive with no usable title: subjects beat an opaque download stem.
        if is_archive_meta(m) {
            if let Some(t) = tag_caption(&m.tags, 3) {
                return t;
            }
        }
    }
    if file_name.trim().is_empty() {
        String::from("Wallpaper")
    } else {
        file_name.to_string()
    }
}

/// Wallhaven ids, URL hashes, `2048x1280-general`, and similar opaque stems.
pub fn looks_like_opaque_stem(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return true;
    }
    if looks_like_resolution_caption(s) || looks_like_resolution_stem(s) {
        return true;
    }
    if (5..=8).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return true;
    }
    if s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    if s.len() > 17
        && s.as_bytes().get(16) == Some(&b'-')
        && s[..16].chars().all(|c| c.is_ascii_hexdigit())
    {
        return true;
    }
    false
}

pub(crate) fn looks_like_resolution_stem(s: &str) -> bool {
    let s = s.replace('×', "x");
    let Some((a, rest)) = s.split_once('x') else {
        return false;
    };
    if a.len() < 3 || !a.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let b = rest
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap_or("");
    b.len() >= 3 && b.chars().all(|c| c.is_ascii_digit())
}

pub(crate) fn wallhaven_preview_complete(item: &RemoteItem) -> bool {
    !item.tags.is_empty()
        && nonempty_opt(&item.uploader).is_some()
        && nonempty_opt(&item.avatar).is_some()
        && item.views.is_some()
        && item.favorites.is_some()
        && nonempty_opt(&item.page_url).is_some()
        && !item.colors.is_empty()
}

pub(crate) fn archive_preview_complete(item: &RemoteItem) -> bool {
    nonempty_opt(&item.page_url).is_some()
        && (nonempty_opt(&item.uploader).is_some()
            || nonempty_opt(&item.date).is_some()
            || nonempty_opt(&item.collection).is_some()
            || nonempty_opt(&item.description).is_some()
            || nonempty_opt(&item.license).is_some()
            || !item.tags.is_empty()
            || item.views.unwrap_or(0) > 0
            || item.downloads.unwrap_or(0) > 0
            || item.duration_secs.is_some_and(|d| d.is_finite() && d > 0.0))
}

pub(crate) fn github_preview_complete(item: &RemoteItem) -> bool {
    nonempty_opt(&item.uploader).is_some()
        && nonempty_opt(&item.page_url).is_some()
        && nonempty_opt(&item.license).is_some()
        && !item.tags.is_empty()
}

pub(crate) fn ensure_archive_page_url(item: &mut RemoteItem) {
    if nonempty_opt(&item.page_url).is_some() {
        return;
    }
    if let Some(id) = item
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        item.page_url = Some(format!("https://archive.org/details/{id}"));
    }
}

pub(crate) fn ensure_github_page_url(item: &mut RemoteItem) {
    if nonempty_opt(&item.page_url).is_some() {
        return;
    }
    let Some(repo) = item
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains('/'))
    else {
        return;
    };
    let path = item
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let n = item.name.trim();
            (!n.is_empty()).then_some(n)
        });
    let Some(path) = path else {
        return;
    };
    item.page_url = Some(format!(
        "https://github.com/{repo}/blob/HEAD/{}",
        percent_encode_path(path.trim_start_matches('/'))
    ));
}

/// Fill Wallhaven / Archive / GitHub details from cache or one HTTP when saving.
pub fn enrich_item_for_save(item: &mut RemoteItem, api_key: &str) {
    if item.url.contains("wallhaven") || item.credit.as_deref() == Some("Wallhaven") {
        if wallhaven_preview_complete(item) {
            return;
        }
        let Some(id) = item
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
        else {
            return;
        };
        if let Ok(d) = fetch_wallhaven_details(&id, api_key) {
            item.apply_wallhaven_details(&d);
        }
        return;
    }
    if item.url.contains("archive.org")
        || item
            .credit
            .as_deref()
            .is_some_and(|c| c.eq_ignore_ascii_case("Internet Archive"))
    {
        ensure_archive_page_url(item);
        if archive_preview_complete(item) {
            return;
        }
        if let Some(id) = item
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
        {
            if let Ok(d) = fetch_archive_details(&id) {
                item.apply_archive_details(&d);
            }
        }
        return;
    }
    if item.repo.is_some() {
        ensure_github_page_url(item);
        if github_preview_complete(item) {
            return;
        }
        if let Ok(d) = fetch_github_details(item, api_key) {
            item.apply_github_details(&d);
        }
    }
}

/// Preview-sidebar stats (Discover listing + local probe + sidecar).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MediaStats {
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub file_size: Option<u64>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub purity: Option<String>,
    #[serde(default)]
    pub uploader: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub collection: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub downloads: Option<u64>,
    #[serde(default)]
    pub views: Option<u64>,
    #[serde(default)]
    pub favorites: Option<u64>,
    #[serde(default)]
    pub comments: Option<u64>,
    #[serde(default)]
    pub page_url: Option<String>,
    #[serde(default)]
    pub colors: Vec<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    /// Canonical source name for stats (`Wallhaven`, `Bing Daily`, …).
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub duration_secs: Option<f64>,
    #[serde(default)]
    pub file_type: Option<String>,
    #[serde(default)]
    pub fps: Option<f32>,
    #[serde(default)]
    pub credit: Option<String>,
    /// Display heading when tags are empty (Bing/NASA/GitHub title, search query, …).
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub source_id: Option<String>,
    /// Looping background music for this wallpaper (absolute path to an audio file).
    #[serde(default)]
    pub bg_music: Option<PathBuf>,
    #[serde(default)]
    pub bg_music_volume: Option<f32>,
    /// Mute background music without clearing `bg_music`.
    #[serde(default)]
    pub bg_music_mute: Option<bool>,
}

impl MediaStats {
    pub fn from_remote(item: &RemoteItem) -> Self {
        let source = item_source_label(item);
        let mut views = item.views;
        let mut downloads = item.downloads;
        // Archive.org Solr `downloads` is the site "Views" engagement counter.
        if source == "Archive.org" && views.unwrap_or(0) == 0 && downloads.unwrap_or(0) > 0 {
            views = downloads.take();
        }
        Self {
            width: item.width,
            height: item.height,
            file_size: item.file_size,
            tags: item.tags.clone(),
            category: item.category.clone(),
            purity: item.purity.clone(),
            uploader: item.uploader.clone(),
            avatar: item.avatar.clone(),
            date: item.date.clone(),
            collection: item.collection.clone(),
            description: item.description.clone(),
            downloads,
            views,
            favorites: item.favorites,
            comments: item.comments,
            page_url: item.page_url.clone(),
            colors: item.colors.clone(),
            repo: item.repo.clone(),
            license: item.license.clone(),
            source: Some(source),
            duration_secs: item.duration_secs,
            file_type: item.file_type.clone(),
            fps: item.fps,
            credit: item.credit.clone(),
            title: remote_display_title(item),
            source_id: item
                .id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            ..Default::default()
        }
    }

    /// Overlay probe/file facts onto catalog metadata (pixels/size/duration win).
    pub fn merge_probe(&mut self, probe: &MediaStats) {
        if probe.width.unwrap_or(0) > 0 {
            self.width = probe.width;
        }
        if probe.height.unwrap_or(0) > 0 {
            self.height = probe.height;
        }
        if probe.file_size.unwrap_or(0) > 0 {
            self.file_size = probe.file_size;
        }
        if probe.duration_secs.is_some_and(|d| d.is_finite() && d > 0.0) {
            self.duration_secs = probe.duration_secs;
        }
        if probe.fps.unwrap_or(0.0) > 0.0 {
            self.fps = probe.fps;
        }
        fill_string(&mut self.file_type, probe.file_type.clone());
        if self.tags.is_empty() && !probe.tags.is_empty() {
            self.tags = probe.tags.clone();
        }
        fill_string(&mut self.category, probe.category.clone());
        fill_string(&mut self.purity, probe.purity.clone());
        fill_string(&mut self.uploader, probe.uploader.clone());
        fill_string(&mut self.avatar, probe.avatar.clone());
        fill_string(&mut self.date, probe.date.clone());
        fill_string(&mut self.collection, probe.collection.clone());
        fill_string(&mut self.description, probe.description.clone());
        if self.downloads.unwrap_or(0) == 0 {
            self.downloads = probe.downloads.filter(|n| *n > 0);
        }
        if self.views.is_none() {
            self.views = probe.views;
        }
        if self.favorites.is_none() {
            self.favorites = probe.favorites;
        }
        if self.comments.is_none() {
            self.comments = probe.comments;
        }
        fill_string(&mut self.page_url, probe.page_url.clone());
        if self.colors.is_empty() && !probe.colors.is_empty() {
            self.colors = probe.colors.clone();
        }
        fill_string(&mut self.repo, probe.repo.clone());
        fill_string(&mut self.license, probe.license.clone());
        fill_string(&mut self.source, probe.source.clone());
        fill_string(&mut self.credit, probe.credit.clone());
    }

    /// Fill blank fields from sidecar / listing metadata.
    pub fn fill_from(&mut self, other: &MediaStats) {
        if self.width.unwrap_or(0) == 0 {
            self.width = other.width;
        }
        if self.height.unwrap_or(0) == 0 {
            self.height = other.height;
        }
        if self.file_size.unwrap_or(0) == 0 {
            self.file_size = other.file_size;
        }
        if self.tags.is_empty() && !other.tags.is_empty() {
            self.tags = other.tags.clone();
        }
        fill_string(&mut self.category, other.category.clone());
        fill_string(&mut self.purity, other.purity.clone());
        fill_string(&mut self.uploader, other.uploader.clone());
        fill_string(&mut self.avatar, other.avatar.clone());
        fill_string(&mut self.date, other.date.clone());
        fill_string(&mut self.collection, other.collection.clone());
        fill_string(&mut self.description, other.description.clone());
        if self.downloads.unwrap_or(0) == 0 {
            self.downloads = other.downloads.filter(|n| *n > 0);
        }
        if self.views.is_none() {
            self.views = other.views;
        }
        if self.favorites.is_none() {
            self.favorites = other.favorites;
        }
        if self.comments.is_none() {
            self.comments = other.comments;
        }
        fill_string(&mut self.page_url, other.page_url.clone());
        if self.colors.is_empty() && !other.colors.is_empty() {
            self.colors = other.colors.clone();
        }
        fill_string(&mut self.repo, other.repo.clone());
        fill_string(&mut self.license, other.license.clone());
        fill_string(&mut self.source, other.source.clone());
        if self.duration_secs.is_none() {
            self.duration_secs = other.duration_secs;
        }
        fill_string(&mut self.file_type, other.file_type.clone());
        if self.fps.unwrap_or(0.0) == 0.0 {
            self.fps = other.fps;
        }
        fill_string(&mut self.credit, other.credit.clone());
        fill_string(&mut self.title, other.title.clone());
        fill_string(&mut self.source_id, other.source_id.clone());
        if self.bg_music.is_none() {
            self.bg_music = other.bg_music.clone();
        }
        if self.bg_music_volume.is_none() {
            self.bg_music_volume = other.bg_music_volume;
        }
        if self.bg_music_mute.is_none() {
            self.bg_music_mute = other.bg_music_mute;
        }
    }

    /// Background music volume for playback (default 0.5).
    pub fn bg_music_volume_or_default(&self) -> f32 {
        self.bg_music_volume.unwrap_or(0.5).clamp(0.0, 1.0)
    }

    pub fn bg_music_muted(&self) -> bool {
        self.bg_music_mute.unwrap_or(false)
    }

    pub(crate) fn wallhaven_preview_incomplete(&self) -> bool {
        self.tags.is_empty()
            || nonempty_opt(&self.uploader).is_none()
            || nonempty_opt(&self.avatar).is_none()
            || self.views.is_none()
            || self.favorites.is_none()
            || nonempty_opt(&self.page_url).is_none()
            || self.colors.is_empty()
    }

    pub(crate) fn archive_preview_incomplete(&self) -> bool {
        nonempty_opt(&self.page_url).is_none()
            || (nonempty_opt(&self.uploader).is_none()
                && nonempty_opt(&self.date).is_none()
                && nonempty_opt(&self.collection).is_none()
                && nonempty_opt(&self.description).is_none()
                && nonempty_opt(&self.license).is_none()
                && self.tags.is_empty()
                && self.views.unwrap_or(0) == 0
                && self.downloads.unwrap_or(0) == 0)
    }

    pub(crate) fn github_preview_incomplete(&self) -> bool {
        nonempty_opt(&self.uploader).is_none()
            || nonempty_opt(&self.page_url).is_none()
            || nonempty_opt(&self.license).is_none()
            || self.tags.is_empty()
    }

    pub fn apply_wallhaven_details(&mut self, d: &WallhavenDetails) {
        if !d.tags.is_empty() {
            self.tags = d.tags.clone();
        }
        fill_string(&mut self.uploader, d.uploader.clone());
        fill_string(&mut self.avatar, d.avatar.clone());
        fill_string(&mut self.purity, d.purity.clone());
        fill_string(&mut self.category, d.category.clone());
        fill_string(&mut self.file_type, d.file_type.clone());
        if self.file_size.unwrap_or(0) == 0 {
            self.file_size = d.file_size.filter(|n| *n > 0);
        }
        if self.width.unwrap_or(0) == 0 {
            self.width = d.width.filter(|n| *n > 0);
        }
        if self.height.unwrap_or(0) == 0 {
            self.height = d.height.filter(|n| *n > 0);
        }
        if d.views.is_some() {
            self.views = d.views;
        }
        if d.favorites.is_some() {
            self.favorites = d.favorites;
        }
        fill_string(&mut self.page_url, d.page_url.clone());
        if self.colors.is_empty() && !d.colors.is_empty() {
            self.colors = d.colors.clone();
        }
        if self.source.is_none() {
            self.source = Some("Wallhaven".into());
        }
        if self.credit.is_none() {
            self.credit = Some("Wallhaven".into());
        }
        if self.title.is_none() {
            self.title = tag_caption(&self.tags, 3);
        }
    }

    pub fn apply_archive_details(&mut self, d: &ArchiveDetails, id: Option<&str>) {
        if let Some(t) = nonempty_opt(&d.title) {
            fill_string(&mut self.title, Some(t.to_string()));
        }
        fill_string(&mut self.uploader, d.creator.clone());
        fill_string(&mut self.category, d.mediatype.clone());
        fill_string(&mut self.file_type, d.file_type.clone());
        fill_string(&mut self.date, d.date.clone());
        fill_string(&mut self.collection, d.collection.clone());
        fill_string(&mut self.description, d.description.clone());
        fill_string(&mut self.license, d.license.clone());
        if !d.tags.is_empty() {
            self.tags = d.tags.clone();
        }
        if self.views.unwrap_or(0) == 0 {
            self.views = d.views.filter(|n| *n > 0).or_else(|| {
                self.downloads.filter(|n| *n > 0)
            });
        }
        if self.views.unwrap_or(0) > 0 {
            self.downloads = None;
        }
        if self.file_size.unwrap_or(0) == 0 {
            self.file_size = d.file_size.filter(|n| *n > 0);
        }
        if self.width.unwrap_or(0) == 0 {
            self.width = d.width.filter(|n| *n > 0);
        }
        if self.height.unwrap_or(0) == 0 {
            self.height = d.height.filter(|n| *n > 0);
        }
        if self.duration_secs.is_none() {
            self.duration_secs = d.duration_secs.filter(|n| n.is_finite() && *n > 0.0);
        }
        if self.source.is_none() {
            self.source = Some("Archive.org".into());
        }
        if self.credit.is_none() {
            self.credit = Some("Internet Archive".into());
        }
        if nonempty_opt(&self.page_url).is_none() {
            if let Some(id) = id.map(str::trim).filter(|s| !s.is_empty()) {
                self.page_url = Some(format!("https://archive.org/details/{id}"));
                fill_string(&mut self.source_id, Some(id.to_string()));
            }
        }
    }

    pub fn apply_github_details(&mut self, d: &GitHubDetails) {
        if self.tags.is_empty() && !d.tags.is_empty() {
            self.tags = d.tags.clone();
        }
        fill_string(&mut self.uploader, d.author.clone());
        fill_string(&mut self.avatar, d.avatar.clone());
        fill_string(&mut self.date, d.date.clone());
        fill_string(&mut self.license, d.license.clone());
        fill_string(&mut self.repo, d.repo.clone());
        if self.width.unwrap_or(0) == 0 {
            self.width = d.width.filter(|n| *n > 0);
        }
        if self.height.unwrap_or(0) == 0 {
            self.height = d.height.filter(|n| *n > 0);
        }
        if self.duration_secs.is_none() {
            self.duration_secs = d.duration_secs.filter(|n| n.is_finite() && *n > 0.0);
        }
        if self.fps.unwrap_or(0.0) == 0.0 {
            self.fps = d.fps.filter(|f| *f > 0.0);
        }
        if self.source.is_none() {
            self.source = Some("GitHub".into());
        }
        if self.credit.is_none() {
            self.credit = Some("GitHub".into());
        }
        if nonempty_opt(&self.page_url).is_none() {
            if let (Some(repo), Some(path)) = (
                nonempty_opt(&self.repo).or(nonempty_opt(&d.repo)),
                nonempty_opt(&self.source_id),
            ) {
                self.page_url = Some(format!(
                    "https://github.com/{repo}/blob/HEAD/{}",
                    percent_encode_path(path.trim_start_matches('/'))
                ));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.format().is_empty()
    }

    /// Dim-label block under Apply / Download. Omits unknown fields.
    pub fn format(&self) -> String {
        self.format_lines(true, true)
    }

    /// Stats text without uploader / tags (those are separate widgets).
    pub fn format_body(&self) -> String {
        self.format_lines(false, false)
    }

    pub(crate) fn format_lines(&self, uploader: bool, tags: bool) -> String {
        let mut lines = Vec::new();
        let source = self
            .source
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                nonempty_opt(&self.credit).map(|c| canonical_source_label(c, None))
            });
        if let Some(s) = &source {
            lines.push(format!("Source: {s}"));
        }
        if let Some(t) = nonempty_opt(&self.title) {
            if tag_caption(&self.tags, 8).as_deref() != Some(t) {
                lines.push(format!("Title: {t}"));
            }
        }
        if let Some(r) = nonempty_opt(&self.repo) {
            lines.push(format!("Repo: {r}"));
        }
        if let Some(id) = nonempty_opt(&self.source_id) {
            if self.repo.is_some() {
                lines.push(format!("Path: {id}"));
            }
        }
        match (self.width, self.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => {
                lines.push(format!("Resolution: {w}×{h}"));
            }
            _ => {}
        }
        if let Some(sz) = self.file_size.filter(|n| *n > 0) {
            lines.push(format!("Size: {}", human_bytes(sz)));
        }
        if let Some(d) = self.duration_secs.filter(|d| d.is_finite() && *d > 0.0) {
            lines.push(format!("Duration: {}", format_duration(d)));
        }
        if let Some(fps) = self.fps.filter(|f| *f > 0.0) {
            let shown = if (fps - fps.round()).abs() < 0.05 {
                format!("{:.0}", fps.round())
            } else {
                format!("{fps:.2}")
            };
            lines.push(format!("FPS: {shown}"));
        }
        if let Some(t) = nonempty_opt(&self.file_type) {
            lines.push(format!("Format: {}", short_file_type(t)));
        }
        if let Some(c) = nonempty_opt(&self.category) {
            lines.push(format!("Category: {}", title_case_ascii(c)));
        }
        if let Some(p) = nonempty_opt(&self.purity) {
            lines.push(format!("Purity: {}", p.to_ascii_uppercase()));
        }
        if let Some(n) = self.views {
            lines.push(format!("Views: {n}"));
        }
        if let Some(n) = self.favorites {
            let label = if self
                .source
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("Pixabay"))
            {
                "Likes"
            } else {
                "Favorites"
            };
            lines.push(format!("{label}: {n}"));
        }
        if let Some(n) = self.comments.filter(|n| *n > 0) {
            lines.push(format!("Comments: {n}"));
        }
        if uploader {
            if let Some(u) = nonempty_opt(&self.uploader) {
                let label = if self.repo.is_some() {
                    "Committed by"
                } else if self
                    .source
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("Archive.org"))
                    || self
                        .credit
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case("Internet Archive"))
                {
                    "Creator"
                } else {
                    "Uploader"
                };
                lines.push(format!("{label}: {u}"));
            }
        }
        if let Some(d) = nonempty_opt(&self.date) {
            lines.push(format!("Date: {d}"));
        }
        if let Some(n) = self.downloads.filter(|n| *n > 0) {
            lines.push(format!("Downloads: {n}"));
        }
        if let Some(c) = nonempty_opt(&self.collection) {
            lines.push(format!("Collection: {c}"));
        }
        if let Some(l) = nonempty_opt(&self.license) {
            lines.push(format!("License: {l}"));
        }
        if let Some(c) = nonempty_opt(&self.credit) {
            let src = source.as_deref().unwrap_or("");
            if !c.eq_ignore_ascii_case(src) && !looks_like_source_credit(c) {
                lines.push(format!("Credit: {c}"));
            }
        }
        if let Some(d) = nonempty_opt(&self.description) {
            lines.push(format!("Description: {d}"));
        }
        if tags {
            if let Some(t) = tag_caption(&self.tags, 10) {
                lines.push(format!("Tags: {t}"));
            }
        }
        lines.join("\n")
    }
}

pub(crate) fn looks_like_source_credit(c: &str) -> bool {
    let l = c.to_ascii_lowercase();
    matches!(
        l.as_str(),
        "wallhaven"
            | "pixabay"
            | "coverr"
            | "bing"
            | "bing daily"
            | "nasa apod"
            | "internet archive"
            | "archive.org"
            | "live wallpapers"
            | "live wallpaper"
            | "video wallpapers"
            | "video wallpaper"
            | "github"
            | "video from coverr"
    ) || l.starts_with("github ")
}

pub fn item_source_label(item: &RemoteItem) -> String {
    canonical_source_label(item.credit.as_deref().unwrap_or(""), Some(&item.url))
}

pub fn canonical_source_label(credit: &str, url: Option<&str>) -> String {
    if let Some(u) = url {
        let u = u.to_ascii_lowercase();
        if u.contains("wallhaven") {
            return "Wallhaven".into();
        }
        if u.contains("archive.org") {
            return "Archive.org".into();
        }
        if u.contains("pixabay") {
            return "Pixabay".into();
        }
        if u.contains("coverr") {
            return "Coverr".into();
        }
        if u.contains("bing.com") {
            return "Bing Daily".into();
        }
        if u.contains("nasa.gov") || u.contains("apod") {
            return "NASA APOD".into();
        }
        if u.contains("github") || u.contains("githubusercontent") {
            return "GitHub".into();
        }
    }
    let c = credit.trim();
    if c.is_empty() {
        return "Library".into();
    }
    let l = c.to_ascii_lowercase();
    if l.contains("wallhaven") {
        "Wallhaven".into()
    } else if l.contains("archive") {
        "Archive.org".into()
    } else if l.contains("pixabay") {
        "Pixabay".into()
    } else if l.contains("coverr") {
        "Coverr".into()
    } else if l.contains("bing") {
        "Bing Daily".into()
    } else if l.contains("nasa") {
        "NASA APOD".into()
    } else if l.contains("github")
        || l.contains("live wallpaper")
        || (l.contains("video") && l.contains("wallpaper"))
    {
        "GitHub".into()
    } else {
        c.to_string()
    }
}

pub(crate) fn nonempty_opt(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

pub fn human_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    let x = n as f64;
    let (val, unit) = if x < KB {
        return format!("{n} B");
    } else if x < KB * KB {
        (x / KB, "KB")
    } else if x < KB * KB * KB {
        (x / (KB * KB), "MB")
    } else {
        (x / (KB * KB * KB), "GB")
    };
    if (val - val.round()).abs() < 0.05 {
        format!("{:.0} {unit}", val.round())
    } else {
        format!("{val:.1} {unit}")
    }
}

pub fn format_duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return String::new();
    }
    let s = secs.round() as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// Category display: `general` → `General` (purity stays FULL CAPS separately).
pub fn title_case_ascii(s: &str) -> String {
    let s = s.trim();
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out = String::with_capacity(s.len());
            out.push(first.to_ascii_uppercase());
            out.extend(chars.map(|c| c.to_ascii_lowercase()));
            out
        }
    }
}

pub(crate) fn short_file_type(raw: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return String::new();
    }
    let lower = s.to_ascii_lowercase();
    let leaf = lower
        .rsplit(['/', '.'])
        .next()
        .unwrap_or(&lower);
    match leaf {
        "jpeg" | "jpg" => "JPEG".into(),
        "png" => "PNG".into(),
        "webp" => "WebP".into(),
        "bmp" => "BMP".into(),
        "gif" => "GIF".into(),
        "mp4" | "m4v" => "MP4".into(),
        "webm" => "WebM".into(),
        "mkv" => "MKV".into(),
        "mov" => "MOV".into(),
        "avi" => "AVI".into(),
        "h264" | "avc" => "H.264".into(),
        "hevc" | "h265" => "H.265".into(),
        "vp9" => "VP9".into(),
        "vp8" => "VP8".into(),
        "av1" => "AV1".into(),
        other => {
            if other.len() <= 8 {
                other.to_ascii_uppercase()
            } else {
                s.to_string()
            }
        }
    }
}

