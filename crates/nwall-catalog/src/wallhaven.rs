use std::fs::File;
use std::io::Read;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use super::*;

pub const WALLHAVEN_RATE_LIMIT_MSG: &str = "Wallhaven rate limit, try again in a moment";

pub(crate) fn fetch_wallhaven(opts: &SearchOpts, api_key: &str) -> Result<FetchResult> {
    let page = opts.page();
    if opts.needs_wallhaven_key() && api_key.trim().is_empty() {
        return Err(anyhow!(
            "Wallhaven NSFW/sketchy needs an API key — add it next to Wallhaven in Settings (wallhaven.cc/settings/account), enable NSFW on the key."
        ));
    }
    let user_query = opts.query.trim();
    let q = if user_query.is_empty() {
        "landscape"
    } else {
        user_query
    };
    let sorting = match opts.sorting.as_str() {
        "date_added" | "relevance" | "random" | "views" | "toplist" => opts.sorting.as_str(),
        _ => "favorites",
    };
    let atleast = opts.atleast.trim();
    let page = if sorting == "random" { 1 } else { page };
    let seed = if sorting == "random" {
        let s = opts.seed.trim();
        if s.is_empty() {
            wallhaven_random_seed()
        } else {
            s.to_string()
        }
    } else {
        String::new()
    };
    let mut result = wallhaven_search(q, user_query, opts, sorting, page, atleast, api_key, &seed)?;
    if result.items.is_empty() && !atleast.is_empty() {
        log::warn!(
            "wallhaven: 0 hits with atleast={atleast} (q={q} sorting={sorting} page={page}); retrying without atleast"
        );
        result = wallhaven_search(q, user_query, opts, sorting, page, "", api_key, &seed)?;
    }
    // Kick off `/w/{id}` tag fetches immediately so titles land while the grid paints.
    begin_wallhaven_detail_warm(&result.items, api_key);
    Ok(result)
}

pub(crate) fn wallhaven_random_seed() -> String {
    let mut buf = [0u8; 8];
    let v = if File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
    {
        u64::from_le_bytes(buf)
    } else {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
    };
    format!("{v:016x}")
}

pub(crate) fn wallhaven_api_get(url: &str, api_key: &str) -> Result<ureq::Response> {
    let has_key = !api_key.trim().is_empty();
    note_source_auth("wallhaven", has_key);
    let class = if url.contains("/api/v1/w/") {
        RequestClass::Detail
    } else {
        RequestClass::Listing
    };
    let mut headers = Vec::new();
    if has_key {
        headers.push(("X-API-Key", api_key.trim()));
    }
    limited_get_headers(url, "wallhaven", class, has_key, &headers)
}

pub(crate) fn wallhaven_search(
    q: &str,
    user_query: &str,
    opts: &SearchOpts,
    sorting: &str,
    page: u32,
    atleast: &str,
    api_key: &str,
    seed: &str,
) -> Result<FetchResult> {
    let mut url = format!(
        "https://wallhaven.cc/api/v1/search?q={}&categories={}&purity={}&sorting={}&order=desc&page={}",
        urlencoding_lite(q),
        opts.wallhaven_categories(),
        opts.wallhaven_purity(),
        sorting,
        page
    );
    if sorting == "random" && !seed.is_empty() {
        url.push_str(&format!("&seed={}", urlencoding_lite(seed)));
    }
    if sorting == "toplist" {
        let range = match opts.top_range.as_str() {
            "1d" | "3d" | "1w" | "3M" | "6M" | "1y" => opts.top_range.as_str(),
            _ => "1M",
        };
        url.push_str(&format!("&topRange={range}"));
    }
    if !atleast.is_empty() {
        url.push_str(&format!("&atleast={}", urlencoding_lite(atleast)));
    }
    let ratios = opts.ratios.trim();
    if !ratios.is_empty() {
        url.push_str(&format!("&ratios={}", urlencoding_lite(ratios)));
    }
    if opts.hide_ai {
        url.push_str("&ai_art_filter=1");
    }
    let parsed: WallhavenSearch = match wallhaven_api_get(&url, api_key) {
        Ok(resp) => match resp.into_json() {
            Ok(p) => p,
            Err(e) => {
                log::warn!("wallhaven json failed: {e}");
                return Err(e).context("wallhaven json");
            }
        },
        Err(e) => {
            log::warn!("wallhaven search failed: {e:#}");
            return Err(e);
        }
    };
    let last_page = parsed.meta.as_ref().and_then(|m| m.last_page);
    let items = parsed
        .data
        .into_iter()
        .map(|w| wallhaven_remote_item(w, user_query))
        .collect();
    Ok(FetchResult {
        items,
        page,
        last_page,
    })
}

pub(crate) fn parse_resolution(s: &str) -> (Option<u32>, Option<u32>) {
    let s = s.trim().to_ascii_lowercase();
    let Some((a, b)) = s.split_once('x') else {
        return (None, None);
    };
    let w = a.trim().parse().ok();
    let h = b.trim().parse().ok();
    (w, h)
}

pub(crate) fn wallhaven_remote_item(w: WallhavenItem, user_query: &str) -> RemoteItem {
    let id = w.id.trim().to_string();
    let details = wallhaven_details_from_item(w.clone());
    let (width, height) = parse_resolution(&w.resolution);
    let width = details.width.or(width);
    let height = details.height.or(height);
    let category = details.category.clone();
    let purity = details.purity.clone();
    let credit = Some("Wallhaven".into());
    let cached = if id.is_empty() {
        None
    } else {
        cached_wallhaven_details(&id)
    };
    let mut tags = details.tags;
    if tags.is_empty() {
        if let Some(c) = &cached {
            tags = c.tags.clone();
        }
    }
    let name = tag_caption(&tags, 3)
        .or_else(|| {
            let q = user_query.trim();
            if q.is_empty() {
                None
            } else {
                Some(q.to_string())
            }
        })
        .or_else(|| {
            details
                .category
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(title_case_ascii)
        })
        .or_else(|| {
            if id.is_empty() {
                None
            } else {
                Some(format!("#{id}"))
            }
        })
        .unwrap_or_else(|| "Wallhaven".into());
    let mut item = RemoteItem {
        name,
        video: false,
        url: w.path,
        thumb: w.thumbs.and_then(|t| t.small.or(t.large)),
        credit,
        id: if id.is_empty() { None } else { Some(id) },
        width,
        height,
        category,
        file_size: details.file_size.or(w.file_size.filter(|n| *n > 0)),
        purity,
        uploader: details.uploader,
        tags,
        file_type: details.file_type,
        views: details.views,
        favorites: details.favorites,
        page_url: details.page_url,
        colors: details.colors,
        ..Default::default()
    };
    if let Some(c) = cached {
        item.apply_wallhaven_details(&c);
    }
    item
}

pub(crate) fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Deserialize)]
pub(crate) struct WallhavenSearch {
    #[serde(default)]
    pub(crate) data: Vec<WallhavenItem>,
    pub(crate) meta: Option<WallhavenMeta>,
}

#[derive(Deserialize)]
pub(crate) struct WallhavenMeta {
    #[serde(default)]
    pub(crate) last_page: Option<u32>,
}

#[derive(Clone, Deserialize, Default)]
pub(crate) struct WallhavenItem {
    #[serde(default)]
    pub(crate) id: String,
    /// Image file URL (downloaded as `RemoteItem.url`).
    #[serde(default)]
    pub(crate) path: String,
    /// Wallpaper page on wallhaven.cc.
    #[serde(default)]
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) short_url: String,
    #[serde(default)]
    pub(crate) views: Option<u64>,
    #[serde(default)]
    pub(crate) favorites: Option<u64>,
    #[serde(default)]
    pub(crate) colors: Vec<String>,
    #[serde(default)]
    pub(crate) resolution: String,
    #[serde(default)]
    pub(crate) category: String,
    #[serde(default)]
    pub(crate) purity: String,
    #[serde(default)]
    pub(crate) file_size: Option<u64>,
    #[serde(default)]
    pub(crate) file_type: String,
    #[serde(default)]
    pub(crate) dimension_x: Option<u32>,
    #[serde(default)]
    pub(crate) dimension_y: Option<u32>,
    pub(crate) thumbs: Option<WallhavenThumbs>,
    pub(crate) uploader: Option<WallhavenUploader>,
    #[serde(default)]
    pub(crate) tags: Vec<WallhavenTag>,
}

#[derive(Clone, Deserialize, Default)]
pub(crate) struct WallhavenTag {
    #[serde(default)]
    pub(crate) name: String,
}

#[derive(Clone, Deserialize, Default)]
pub(crate) struct WallhavenUploader {
    #[serde(default)]
    pub(crate) username: String,
    pub(crate) avatar: Option<WallhavenAvatar>,
}

#[derive(Clone, Deserialize, Default)]
pub(crate) struct WallhavenAvatar {
    #[serde(rename = "32px", default)]
    pub(crate) px32: Option<String>,
    #[serde(rename = "128px", default)]
    pub(crate) px128: Option<String>,
    #[serde(rename = "200px", default)]
    pub(crate) px200: Option<String>,
}

#[derive(Clone, Deserialize, Default)]
pub(crate) struct WallhavenThumbs {
    pub(crate) small: Option<String>,
    pub(crate) large: Option<String>,
}
