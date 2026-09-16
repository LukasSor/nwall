//! Persisted Discover search filters (per source kind).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscoverFiltersState {
    #[serde(default)]
    pub wallhaven: WallhavenFiltersState,
    #[serde(default)]
    pub pixabay: PixabayFiltersState,
    #[serde(default)]
    pub coverr: CoverrFiltersState,
    #[serde(default)]
    pub archive: ArchiveFiltersState,
    #[serde(default)]
    pub github: GithubFiltersState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallhavenFiltersState {
    #[serde(default = "true_default")]
    pub sfw: bool,
    #[serde(default)]
    pub sketchy: bool,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(default = "true_default")]
    pub cat_general: bool,
    #[serde(default = "true_default")]
    pub cat_anime: bool,
    #[serde(default = "true_default")]
    pub cat_people: bool,
    #[serde(default)]
    pub sort: u32,
    #[serde(default = "default_top_range")]
    pub top_range: u32,
    #[serde(default)]
    pub atleast: u32,
    #[serde(default)]
    pub ratios: u32,
    #[serde(default)]
    pub hide_ai: bool,
    /// Load the full wallpaper for the sidebar preview (overrides Settings → fast image).
    #[serde(default = "true_default")]
    pub load_full_preview: bool,
}

impl Default for WallhavenFiltersState {
    fn default() -> Self {
        Self {
            sfw: true,
            sketchy: false,
            nsfw: false,
            cat_general: true,
            cat_anime: true,
            cat_people: true,
            sort: 0,
            top_range: 3,
            atleast: 0,
            ratios: 0,
            hide_ai: false,
            load_full_preview: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PixabayFiltersState {
    #[serde(default = "true_default")]
    pub safesearch: bool,
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub media: u32,
    #[serde(default)]
    pub video_type: u32,
    #[serde(default)]
    pub category: u32,
}

impl Default for PixabayFiltersState {
    fn default() -> Self {
        Self {
            safesearch: true,
            order: 0,
            media: 0,
            video_type: 0,
            category: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverrFiltersState {
    #[serde(default)]
    pub sort: u32,
}

impl Default for CoverrFiltersState {
    fn default() -> Self {
        Self { sort: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveFiltersState {
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub media: u32,
}

impl Default for ArchiveFiltersState {
    fn default() -> Self {
        Self { order: 0, media: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubFiltersState {
    #[serde(default)]
    pub media: u32,
    /// Selected `owner/repo` names. Empty = all repos in the pack.
    #[serde(default)]
    pub repos: Vec<String>,
}

impl Default for GithubFiltersState {
    fn default() -> Self {
        Self {
            media: 0,
            repos: Vec::new(),
        }
    }
}

fn true_default() -> bool {
    true
}

fn default_top_range() -> u32 {
    3
}
