pub mod discover_filters;
pub mod icon;

pub use discover_filters::DiscoverFiltersState;
pub use icon::nwall_icon_rgba;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

pub const LIVE_NAMESPACE: &str = "nwall-live";
pub const BACKDROP_NAMESPACE: &str = "nwall-backdrop";

pub fn socket_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("nwall.sock");
    }
    let uid = libc_uid();
    PathBuf::from(format!("/tmp/nwall-{uid}.sock"))
}

fn libc_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

pub fn cache_dir() -> PathBuf {
    let mut dir = if let Some(c) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(c)
    } else {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".cache")
    };
    dir.push("nwall");
    dir.push("remote");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn daemon_log_path() -> PathBuf {
    let mut dir = if let Some(c) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(c)
    } else {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".cache")
    };
    dir.push("nwall");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("nwalld.log")
}

pub fn hash_url(url: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("nwall");
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(".config").join("nwall")
}

pub fn default_config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn default_library_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Pictures").join("Wallpapers");
    }
    PathBuf::from("Wallpapers")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FitMode {
    #[default]
    Cover,
    Contain,
    Stretch,
}

impl FitMode {
    pub fn as_str(self) -> &'static str {
        match self {
            FitMode::Cover => "cover",
            FitMode::Contain => "contain",
            FitMode::Stretch => "stretch",
        }
    }
}

impl std::str::FromStr for FitMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "cover" => Ok(FitMode::Cover),
            "contain" | "fit" => Ok(FitMode::Contain),
            "stretch" => Ok(FitMode::Stretch),
            other => Err(anyhow!("unknown fit mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PausePolicy {
    #[serde(default = "true_default")]
    pub on_fullscreen: bool,
    #[serde(default = "true_default")]
    pub on_overview: bool,
    #[serde(default = "true_default")]
    pub on_window_drag: bool,
    #[serde(default = "true_default")]
    pub on_covered: bool,
    #[serde(default = "true_default")]
    pub music_on_fullscreen: bool,
    #[serde(default = "true_default")]
    pub music_on_overview: bool,
    #[serde(default)]
    pub music_on_window_drag: bool,
    #[serde(default)]
    pub music_on_covered: bool,
    #[serde(default = "true_default")]
    pub music_on_other_audio: bool,
}

impl Default for PausePolicy {
    fn default() -> Self {
        Self {
            on_fullscreen: true,
            on_overview: true,
            on_window_drag: false,
            on_covered: false,
            music_on_fullscreen: true,
            music_on_overview: true,
            music_on_window_drag: false,
            music_on_covered: false,
            music_on_other_audio: true,
        }
    }
}

impl PausePolicy {
    pub fn any_music_niri_trigger(&self) -> bool {
        self.music_on_fullscreen
            || self.music_on_overview
            || self.music_on_window_drag
            || self.music_on_covered
    }
}

fn true_default() -> bool {
    true
}

fn fps_default() -> u32 {
    24
}

fn mute_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_library_dir")]
    pub library: PathBuf,
    #[serde(default)]
    pub wallpaper: Option<PathBuf>,
    #[serde(default)]
    pub outputs: std::collections::HashMap<String, PathBuf>,
    #[serde(default = "fps_default")]
    pub fps: u32,
    #[serde(default = "mute_default")]
    pub mute: bool,
    #[serde(default)]
    pub volume: f32,
    #[serde(default)]
    pub fit: FitMode,
    #[serde(default)]
    pub pause: PausePolicy,
    #[serde(default = "theme_default")]
    pub theme: String,
    #[serde(default = "true_default")]
    pub show_stats: bool,
    #[serde(default = "true_default")]
    pub fast_image_preview: bool,
    #[serde(default)]
    pub fast_video_preview: bool,
    #[serde(default = "true_default")]
    pub show_monitors: bool,
    #[serde(default = "preview_width_default")]
    pub preview_width: f64,
    #[serde(default = "gui_zoom_default")]
    pub gui_zoom: f64,
    #[serde(default)]
    pub theme_css: Option<PathBuf>,
    #[serde(default = "default_sources")]
    pub sources: Vec<CatalogSource>,
    #[serde(default)]
    pub slideshow: Slideshow,
    #[serde(default)]
    pub discover_github_repos: Vec<String>,
    #[serde(default)]
    pub discover_filters: DiscoverFiltersState,
}

fn default_slideshow_interval() -> u32 {
    30
}

fn default_slideshow_source() -> String {
    "Wallhaven".into()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slideshow {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_slideshow_interval")]
    pub interval_minutes: u32,
    #[serde(default = "default_slideshow_source")]
    pub source: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default = "default_true")]
    pub show_in_tray: bool,
}

impl Default for Slideshow {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: default_slideshow_interval(),
            source: default_slideshow_source(),
            tags: String::new(),
            show_in_tray: true,
        }
    }
}

impl Slideshow {
    pub fn clamp_interval(&mut self) {
        self.interval_minutes = self.interval_minutes.clamp(1, 1440);
    }
}

fn theme_default() -> String {
    "system".into()
}

fn preview_width_default() -> f64 {
    28.0
}

pub fn normalize_preview_width_pct(v: f64) -> f64 {
    if !v.is_finite() || v <= 0.0 {
        return preview_width_default();
    }
    if v <= 1.0 {
        return (v * 100.0).clamp(20.0, 50.0);
    }
    if v <= 50.0 {
        return v.clamp(20.0, 50.0);
    }
    preview_width_default()
}

fn gui_zoom_default() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSource {
    pub name: String,
    #[serde(default = "index_kind")]
    pub kind: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub repos: Vec<GithubRepo>,
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubRepo {
    pub repo: String,
    #[serde(default)]
    pub path: String,
}

impl CatalogSource {
    pub fn github_targets(&self) -> Vec<(String, String)> {
        if !self.repos.is_empty() {
            return self
                .repos
                .iter()
                .filter(|r| !r.repo.trim().is_empty())
                .map(|r| (r.repo.trim().to_string(), r.path.trim().to_string()))
                .collect();
        }
        let repo = self.repo.trim();
        if repo.is_empty() {
            Vec::new()
        } else {
            vec![(repo.to_string(), self.path.trim().to_string())]
        }
    }
}

fn index_kind() -> String {
    "index".into()
}

fn builtin(name: &str, kind: &str) -> CatalogSource {
    CatalogSource {
        name: name.into(),
        kind: kind.into(),
        url: String::new(),
        repo: String::new(),
        path: String::new(),
        repos: Vec::new(),
        api_key: String::new(),
    }
}

fn default_github_repos() -> Vec<GithubRepo> {
    vec![
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "animated".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "nature".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "mountain".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "minimal".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "abstract".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "aerial".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "gruvbox".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "outrun".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "pixel".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "anime".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "nord".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "digital".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "monochrome".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "architecture".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "flowers".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "fogsmoke".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "fauna".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "manga".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "retro".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "calm".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "dreamcore".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "evangelion".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "cold".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "decay".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "painting".into(),
        },
        GithubRepo {
            repo: "dharmx/walls".into(),
            path: "cherry".into(),
        },
        GithubRepo {
            repo: "JoshuaThadi/Wall-E-Desk".into(),
            path: "Live Wallpapers".into(),
        },
        GithubRepo {
            repo: "JoshuaThadi/Wall-E-Desk".into(),
            path: "Anime".into(),
        },
        GithubRepo {
            repo: "JoshuaThadi/Wall-E-Desk".into(),
            path: "Pixel-Art".into(),
        },
        GithubRepo {
            repo: "JoshuaThadi/Wall-E-Desk".into(),
            path: "Sci-Fi".into(),
        },
        GithubRepo {
            repo: "JoshuaThadi/Wall-E-Desk".into(),
            path: "landscape-anime".into(),
        },
        GithubRepo {
            repo: "JoshuaThadi/Wall-E-Desk".into(),
            path: "landscape-nature".into(),
        },
        GithubRepo {
            repo: "SniperRavan/Wallpapers".into(),
            path: "My-laptop-wallpaper".into(),
        },
        GithubRepo {
            repo: "SniperRavan/Wallpapers".into(),
            path: "LOCK-SCREEN".into(),
        },
        GithubRepo {
            repo: "SniperRavan/Wallpapers".into(),
            path: "SLIDE-SHOWING-WALLPAPER".into(),
        },
        GithubRepo {
            repo: "SniperRavan/Wallpapers".into(),
            path: "pixalated".into(),
        },
        GithubRepo {
            repo: "vyrx-dev/Wallpapers".into(),
            path: "calm".into(),
        },
        GithubRepo {
            repo: "vyrx-dev/Wallpapers".into(),
            path: "digital".into(),
        },
        GithubRepo {
            repo: "vyrx-dev/Wallpapers".into(),
            path: "gruvbox".into(),
        },
        GithubRepo {
            repo: "vyrx-dev/Wallpapers".into(),
            path: "monochrome".into(),
        },
        GithubRepo {
            repo: "vyrx-dev/Wallpapers".into(),
            path: "nord".into(),
        },
        GithubRepo {
            repo: "vyrx-dev/Wallpapers".into(),
            path: "others".into(),
        },
        GithubRepo {
            repo: "vyrx-dev/Wallpapers".into(),
            path: "Vertical-wallpapers".into(),
        },
        GithubRepo {
            repo: "mylinuxforwork/wallpaper".into(),
            path: String::new(),
        },
        GithubRepo {
            repo: "adiambassador/wallpapers".into(),
            path: "abstract".into(),
        },
        GithubRepo {
            repo: "adiambassador/wallpapers".into(),
            path: "genshin".into(),
        },
        GithubRepo {
            repo: "adiambassador/wallpapers".into(),
            path: "honkai".into(),
        },
        GithubRepo {
            repo: "adiambassador/wallpapers".into(),
            path: "jjk".into(),
        },
        GithubRepo {
            repo: "adiambassador/wallpapers".into(),
            path: "arknight".into(),
        },
        GithubRepo {
            repo: "adiambassador/wallpapers".into(),
            path: "goddess_of_victory".into(),
        },
        GithubRepo {
            repo: "adiambassador/wallpapers".into(),
            path: "wuthering_waves".into(),
        },
        GithubRepo {
            repo: "adiambassador/wallpapers".into(),
            path: "not_yet_categorized".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "animated/abstract".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "animated/cars".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "animated/characters".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "animated/misc".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "animated/scenery".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "static/abstract".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "static/automotive".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "static/characters".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "static/coding".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "static/colors".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "static/minecraft".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "static/misc".into(),
        },
        GithubRepo {
            repo: "usman-369/wallpapers".into(),
            path: "static/scenery".into(),
        },
        GithubRepo {
            repo: "linuxdotexe/nordic-wallpapers".into(),
            path: "wallpapers".into(),
        },
        GithubRepo {
            repo: "JaKooLit/Wallpaper-Bank".into(),
            path: "wallpapers".into(),
        },
        GithubRepo {
            repo: "FrenzyExists/wallpapers".into(),
            path: "Anime".into(),
        },
        GithubRepo {
            repo: "FrenzyExists/wallpapers".into(),
            path: "Pixelart".into(),
        },
        GithubRepo {
            repo: "FrenzyExists/wallpapers".into(),
            path: "Gruv".into(),
        },
        GithubRepo {
            repo: "FrenzyExists/wallpapers".into(),
            path: "Nord".into(),
        },
        GithubRepo {
            repo: "FrenzyExists/wallpapers".into(),
            path: "Monochrome".into(),
        },
        GithubRepo {
            repo: "FrenzyExists/wallpapers".into(),
            path: "Other".into(),
        },
        GithubRepo {
            repo: "FrenzyExists/wallpapers".into(),
            path: "Cherry Blossoms".into(),
        },
    ]
}

fn default_github_source() -> CatalogSource {
    CatalogSource {
        name: "GitHub".into(),
        kind: "github".into(),
        url: String::new(),
        repo: String::new(),
        path: String::new(),
        repos: default_github_repos(),
        api_key: String::new(),
    }
}

fn is_legacy_live_github(s: &CatalogSource) -> bool {
    if !s.kind.eq_ignore_ascii_case("github") || !s.repos.is_empty() {
        return false;
    }
    matches!(
        (s.repo.as_str(), s.path.as_str()),
        ("dharmx/walls", "animated")
            | ("JoshuaThadi/Wall-E-Desk", "Live Wallpapers")
            | ("SniperRavan/Wallpapers", "My-laptop-wallpaper")
    )
}

fn is_legacy_github_pack_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "live wallpapers"
            | "live wallpaper"
            | "video wallpapers"
            | "video wallpaper"
            | "live videos"
            | "live wallpapers (wall-e-desk)"
            | "live pack (sniperravan)"
    )
}

fn is_builtin_github_source(s: &CatalogSource) -> bool {
    if !s.kind.eq_ignore_ascii_case("github") {
        return false;
    }
    let n = s.name.trim();
    n.eq_ignore_ascii_case("GitHub") || is_legacy_github_pack_name(n)
}

fn default_sources() -> Vec<CatalogSource> {
    vec![
        builtin("Wallhaven", "wallhaven"),
        default_github_source(),
        builtin("Pixabay", "pixabay"),
        builtin("Coverr", "coverr"),
        builtin("Archive.org", "archive"),
        builtin("Bing Daily", "bing"),
        builtin("NASA APOD", "nasa"),
    ]
}

fn canonical_source_kind_order(kind: &str) -> u32 {
    match kind.to_ascii_lowercase().as_str() {
        "wallhaven" => 0,
        "github" => 1,
        "pixabay" => 2,
        "coverr" => 3,
        "archive" => 4,
        "bing" => 5,
        "nasa" => 6,
        _ => 100,
    }
}

fn sort_sources_canonical(sources: &mut Vec<CatalogSource>) {
    sources.sort_by(|a, b| {
        let oa = canonical_source_kind_order(&a.kind);
        let ob = canonical_source_kind_order(&b.kind);
        oa.cmp(&ob).then_with(|| a.name.cmp(&b.name))
    });
}

fn sources_equivalent(a: &CatalogSource, b: &CatalogSource) -> bool {
    a.kind == b.kind
        && a.name == b.name
        && a.repo == b.repo
        && a.path == b.path
        && a.url == b.url
        && a.repos == b.repos
}

fn migrate_discover_filters(cfg: &mut Config) {
    if cfg.discover_filters.github.repos.is_empty() && !cfg.discover_github_repos.is_empty() {
        cfg.discover_filters.github.repos = cfg.discover_github_repos.clone();
    }
}

fn migrate_pixabay_source_name(sources: &mut Vec<CatalogSource>) {
    for s in sources.iter_mut() {
        if !s.kind.eq_ignore_ascii_case("pixabay") {
            continue;
        }
        let n = s.name.trim();
        if n.eq_ignore_ascii_case("Pixabay Videos") || n.eq_ignore_ascii_case("Pixabay Video") {
            s.name = "Pixabay".into();
        }
    }
}

fn migrate_github_pack_source(sources: &mut Vec<CatalogSource>) {
    let defaults = default_github_repos();
    for s in sources.iter_mut() {
        if !s.kind.eq_ignore_ascii_case("github") {
            continue;
        }
        let was_legacy_name = is_legacy_github_pack_name(&s.name);
        if was_legacy_name {
            s.name = "GitHub".into();
        }
        if !s.name.eq_ignore_ascii_case("GitHub") {
            continue;
        }
        if s.repos.is_empty() && !s.repo.trim().is_empty() {
            s.repos.push(GithubRepo {
                repo: s.repo.trim().to_string(),
                path: s.path.trim().to_string(),
            });
            s.repo.clear();
            s.path.clear();
        }
        for d in &defaults {
            if !s
                .repos
                .iter()
                .any(|r| r.repo == d.repo && r.path == d.path)
            {
                s.repos.push(d.clone());
            }
        }
    }
}

pub fn ensure_builtin_sources(sources: &mut Vec<CatalogSource>) {
    sources.retain(|s| !is_legacy_live_github(s));
    migrate_pixabay_source_name(sources);
    migrate_github_pack_source(sources);
    for b in default_sources() {
        if sources.iter().any(|s| sources_equivalent(s, &b)) {
            continue;
        }
        if is_builtin_github_source(&b) && sources.iter().any(is_builtin_github_source) {
            continue;
        }
        if b.kind.eq_ignore_ascii_case("pixabay")
            && sources
                .iter()
                .any(|s| s.kind.eq_ignore_ascii_case("pixabay"))
        {
            continue;
        }
        sources.push(b);
    }
    sort_sources_canonical(sources);
}

pub fn canonicalize_source_name(name: &str) -> String {
    let n = name.trim();
    if is_legacy_github_pack_name(n) {
        return "GitHub".into();
    }
    if n.eq_ignore_ascii_case("Pixabay Videos") || n.eq_ignore_ascii_case("Pixabay Video") {
        return "Pixabay".into();
    }
    n.to_string()
}

pub fn is_library_source(name: &str) -> bool {
    let n = name.trim();
    n.eq_ignore_ascii_case("library") || n.eq_ignore_ascii_case("Library (local files)")
}

pub fn resolve_catalog_source<'a>(
    sources: &'a [CatalogSource],
    name: &str,
) -> Option<&'a CatalogSource> {
    let n = canonicalize_source_name(name);
    if n.is_empty() {
        return sources
            .iter()
            .find(|s| s.kind.eq_ignore_ascii_case("bing"))
            .or_else(|| {
                sources
                    .iter()
                    .find(|s| s.kind.eq_ignore_ascii_case("wallhaven"))
            });
    }
    sources
        .iter()
        .find(|s| s.name == n)
        .or_else(|| sources.iter().find(|s| s.name.eq_ignore_ascii_case(&n)))
        .or_else(|| {
            let lower = n.to_ascii_lowercase();
            if lower == "bing" || lower == "bing daily" || lower.contains("bing") {
                sources
                    .iter()
                    .find(|s| s.kind.eq_ignore_ascii_case("bing"))
            } else if lower == "wallhaven" || lower.contains("wallhaven") {
                sources
                    .iter()
                    .find(|s| s.kind.eq_ignore_ascii_case("wallhaven"))
            } else if lower == "github"
                || (lower.contains("live") && lower.contains("wallpaper"))
                || (lower.contains("video") && lower.contains("wallpaper"))
            {
                sources.iter().find(|s| is_builtin_github_source(s))
            } else if lower == "pixabay" || lower.contains("pixabay") {
                sources
                    .iter()
                    .find(|s| s.kind.eq_ignore_ascii_case("pixabay"))
            } else {
                None
            }
        })
}

impl Default for Config {
    fn default() -> Self {
        Self {
            library: default_library_dir(),
            wallpaper: None,
            outputs: Default::default(),
            fps: 24,
            mute: true,
            volume: 0.0,
            fit: FitMode::Cover,
            pause: PausePolicy::default(),
            theme: theme_default(),
            show_stats: true,
            fast_image_preview: true,
            fast_video_preview: false,
            show_monitors: true,
            preview_width: preview_width_default(),
            gui_zoom: gui_zoom_default(),
            theme_css: None,
            sources: default_sources(),
            slideshow: Slideshow::default(),
            discover_github_repos: Vec::new(),
            discover_filters: DiscoverFiltersState::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text).context("parse config.toml")?;
        ensure_builtin_sources(&mut cfg.sources);
        cfg.slideshow.source = canonicalize_source_name(&cfg.slideshow.source);
        if cfg.slideshow.source.trim().is_empty() {
            cfg.slideshow.source = default_slideshow_source();
        }
        cfg.slideshow.clamp_interval();
        migrate_discover_filters(&mut cfg);
        cfg.clamp_gui();
        cfg.library = expand_tilde(&cfg.library);
        if let Some(wp) = cfg.wallpaper.take() {
            cfg.wallpaper = Some(expand_tilde(&wp));
        }
        if let Some(css) = cfg.theme_css.take() {
            cfg.theme_css = Some(expand_tilde(&css));
        }
        cfg.outputs = cfg
            .outputs
            .into_iter()
            .map(|(k, v)| (k, expand_tilde(&v)))
            .collect();
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = self.clone();
        out.clamp_gui();
        let text = toml::to_string_pretty(&out).context("serialize config")?;
        std::fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    fn clamp_gui(&mut self) {
        self.preview_width = normalize_preview_width_pct(self.preview_width);
        if !self.gui_zoom.is_finite() {
            self.gui_zoom = 1.0;
        }
        self.gui_zoom = (self.gui_zoom * 100.0).round() / 100.0;
        self.gui_zoom = self.gui_zoom.clamp(0.75, 1.75);
    }

    pub fn save_preserving_gui(&mut self, path: &Path) -> Result<()> {
        if let Ok(disk) = Self::load(path) {
            self.show_stats = disk.show_stats;
            self.fast_image_preview = disk.fast_image_preview;
            self.fast_video_preview = disk.fast_video_preview;
            self.show_monitors = disk.show_monitors;
            self.preview_width = disk.preview_width;
            self.gui_zoom = disk.gui_zoom;
            self.discover_github_repos = disk.discover_github_repos;
            self.discover_filters = disk.discover_filters;
        }
        self.save(path)
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(&s[2..]);
        }
    }
    path.to_path_buf()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    Set {
        path: PathBuf,
        #[serde(default)]
        output: Option<String>,
        #[serde(default)]
        outputs: Vec<String>,
    },
    SetFps {
        fps: u32,
    },
    SetMute {
        mute: bool,
    },
    SetVolume {
        volume: f32,
    },
    SetFit {
        fit: FitMode,
    },
    SetPausePolicy {
        pause: PausePolicy,
    },
    SetSlideshow {
        slideshow: Slideshow,
    },
    SlideshowStart,
    SlideshowStop,
    Pause,
    Resume,
    ReloadConfig,
    Quit,
    SetBgMusic {
        wallpaper: PathBuf,
        music: Option<PathBuf>,
    },
    SetBgMusicVolume {
        wallpaper: PathBuf,
        volume: f32,
    },
    SetBgMusicMute {
        #[serde(default)]
        wallpaper: Option<PathBuf>,
        mute: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputStatus {
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    pub wallpaper: Option<PathBuf>,
    pub frozen: bool,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub running: bool,
    pub fps: u32,
    pub mute: bool,
    pub volume: f32,
    pub fit: FitMode,
    pub pause: PausePolicy,
    pub globally_frozen: bool,
    pub freeze_reason: Option<String>,
    pub outputs: Vec<OutputStatus>,
    #[serde(default)]
    pub bg_music_active: bool,
    #[serde(default)]
    pub bg_music_mute: bool,
    #[serde(default)]
    pub bg_music_volume: f32,
    #[serde(default)]
    pub slideshow: Slideshow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Ok { ok: bool, message: Option<String> },
    Status(Status),
    Error { error: String },
}

impl Response {
    pub fn ok_msg(msg: impl Into<String>) -> Self {
        Self::Ok {
            ok: true,
            message: Some(msg.into()),
        }
    }

    pub fn ok() -> Self {
        Self::Ok {
            ok: true,
            message: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self::Error {
            error: msg.into(),
        }
    }
}

pub fn client_request(req: &Request) -> Result<Response> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("connect to {} — is nwalld running?", path.display()))?;
    let line = serde_json::to_string(req)? + "\n";
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut response = String::new();
    BufReader::new(&stream).read_line(&mut response)?;
    let response = response.trim();
    if response.is_empty() {
        return Err(anyhow!("empty response from daemon"));
    }
    Ok(serde_json::from_str(response)?)
}

pub fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "bmp")
    )
}

pub fn is_video(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("mp4" | "webm" | "mkv" | "avi" | "mov" | "m4v" | "gif")
    )
}

pub fn is_audio(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("mp3" | "ogg" | "oga" | "flac" | "m4a" | "aac" | "wav" | "opus" | "wma")
    )
}
