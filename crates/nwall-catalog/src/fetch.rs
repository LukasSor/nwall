use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use nwall_ipc::CatalogSource;
use super::*;

pub fn fetch_source(src: &CatalogSource, opts: &SearchOpts) -> Result<FetchResult> {
    // Random results must not stick to a 6h cache of the same 24 hits.
    let cacheable = !opts.sorting.eq_ignore_ascii_case("random");

    // GitHub: cache full listing once; filter query/media locally.
    if src.kind.eq_ignore_ascii_case("github") {
        return fetch_github_catalog(src, opts, cacheable);
    }

    if cacheable {
        if let Some(ttl) = cache_ttl(&src.kind) {
            if let Some(cached) = read_search_cache(src, opts, ttl) {
                return Ok(cached);
            }
        }
    }
    let result = match src.kind.as_str() {
        "wallhaven" => wallhaven::fetch_wallhaven(opts, &source_api_key(src))?,
        "bing" => FetchResult::unpaged(filter_by_query(bing::fetch_bing()?, &opts.query)),
        "nasa" => FetchResult::unpaged(filter_by_query(nasa::fetch_nasa(&source_api_key(src))?, &opts.query)),
        "pixabay" => pixabay::fetch_pixabay(opts, &source_api_key(src))?,
        "coverr" => coverr::fetch_coverr(opts, &source_api_key(src))?,
        "archive" => archive::fetch_archive(opts)?,
        _ => {
            if src.url.is_empty() {
                return Err(anyhow!("catalog '{}' has no url", src.name));
            }
            FetchResult::unpaged(filter_by_query(index::fetch_index(&src.url)?, &opts.query))
        }
    };
    if cacheable && cache_ttl(&src.kind).is_some() {
        write_search_cache(src, opts, &result);
    }
    Ok(result)
}

pub(crate) fn fetch_github_catalog(
    src: &CatalogSource,
    opts: &SearchOpts,
    cacheable: bool,
) -> Result<FetchResult> {
    let mut list_opts = opts.clone();
    list_opts.query.clear();
    list_opts.media = "all".into();
    list_opts.github_repos.clear();
    list_opts.page = 1;
    let raw = if cacheable {
        if let Some(ttl) = cache_ttl("github") {
            if let Some(cached) = read_search_cache(src, &list_opts, ttl) {
                cached.items
            } else {
                match github::fetch_github_source(src) {
                    Ok(items) => {
                        write_search_cache(src, &list_opts, &FetchResult::unpaged(items.clone()));
                        items
                    }
                    Err(e) => {
                        // Rate limits / outages: keep Discover usable with last good list.
                        if let Some(stale) = read_search_cache_any_age(src, &list_opts) {
                            log::warn!(
                                "github fetch failed ({e:#}); using stale cache ({} items)",
                                stale.items.len()
                            );
                            stale.items
                        } else {
                            return Err(e);
                        }
                    }
                }
            }
        } else {
            github::fetch_github_source(src)?
        }
    } else {
        github::fetch_github_source(src)?
    };
    let filtered = filter_by_media(
        filter_by_query(filter_by_github_repos(raw, &opts.github_repos), &opts.query),
        &opts.media,
    );
    Ok(page_github_items(filtered, opts.page()))
}

/// Slice the full GitHub list into Discover pages (thumbs load for this page only).
pub(crate) fn page_github_items(items: Vec<RemoteItem>, page: u32) -> FetchResult {
    let total = items.len() as u32;
    let per_page = GITHUB_PAGE_SIZE;
    let last_page = last_page_from_count(total, per_page).unwrap_or(1).max(1);
    let page = page.max(1).min(last_page);
    let start = ((page - 1) as usize).saturating_mul(per_page as usize);
    let end = (start + per_page as usize).min(items.len());
    let page_items = if start >= items.len() {
        Vec::new()
    } else {
        items[start..end].to_vec()
    };
    FetchResult {
        items: page_items,
        page,
        last_page: Some(last_page),
    }
}

pub(crate) fn cache_ttl(kind: &str) -> Option<std::time::Duration> {
    match kind {
        "pixabay" | "coverr" => Some(std::time::Duration::from_secs(24 * 3600)),
        "wallhaven" | "bing" | "nasa" | "archive" | "github" | "index" => {
            Some(std::time::Duration::from_secs(6 * 3600))
        }
        _ => None,
    }
}

/// Bump when cached `RemoteItem` shape / listing strategy changes (invalidates old JSON).
pub(crate) const SEARCH_CACHE_VER: &str = "list15";

pub(crate) fn search_cache_file(src: &CatalogSource, opts: &SearchOpts) -> PathBuf {
    let dir = cache_dir().join("search");
    let _ = std::fs::create_dir_all(&dir);
    let key = format!(
        "{}|{}|{}|{}|{}|{}|repos{}|p{}|pur{}{}{}|cat{}{}{}|{}|tr{}|{}|r{}|ai{}|ss{}|{}|c{}|vt{}|m{}|ghsel{}",
        SEARCH_CACHE_VER,
        src.kind,
        src.repo,
        src.path,
        src.url,
        opts.query,
        github_repos_cache_key(src),
        opts.page(),
        u8::from(opts.purity_sfw),
        u8::from(opts.purity_sketchy),
        u8::from(opts.purity_nsfw),
        u8::from(opts.cat_general),
        u8::from(opts.cat_anime),
        u8::from(opts.cat_people),
        opts.sorting,
        opts.top_range,
        opts.atleast,
        opts.ratios,
        u8::from(opts.hide_ai),
        u8::from(opts.safesearch),
        opts.order,
        opts.category,
        opts.video_type,
        opts.media,
        opts.github_repos.join(",")
    );
    dir.join(format!("{}.json", hash_url(&key)))
}

pub(crate) fn read_search_cache(
    src: &CatalogSource,
    opts: &SearchOpts,
    ttl: std::time::Duration,
) -> Option<FetchResult> {
    let path = search_cache_file(src, opts);
    let meta = path.metadata().ok()?;
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age > ttl {
        return None;
    }
    read_search_cache_file(&path)
}

/// Same as `read_search_cache` but ignores TTL (rate-limit / offline fallback).
pub(crate) fn read_search_cache_any_age(src: &CatalogSource, opts: &SearchOpts) -> Option<FetchResult> {
    let path = search_cache_file(src, opts);
    if !path.is_file() {
        return None;
    }
    read_search_cache_file(&path)
}

pub(crate) fn read_search_cache_file(path: &Path) -> Option<FetchResult> {
    let text = std::fs::read_to_string(path).ok()?;
    let result: FetchResult = serde_json::from_str(&text).ok()?;
    // Reject caches that stored Wallhaven ids or `1920×1080 · general` as the caption.
    if cache_has_stale_wallhaven_names(&result) {
        return None;
    }
    // Reject Bing/NASA caches missing date/description/page_url/file_type.
    if cache_has_incomplete_daily_meta(&result) {
        return None;
    }
    if result.items.is_empty() {
        return None;
    }
    Some(result)
}

pub(crate) fn looks_like_resolution_caption(s: &str) -> bool {
    let head = s.trim().split(" · ").next().unwrap_or("").trim();
    let sep = if head.contains('×') {
        '×'
    } else if head.contains('x') {
        'x'
    } else {
        return false;
    };
    let Some((a, b)) = head.split_once(sep) else {
        return false;
    };
    !a.is_empty()
        && a.chars().all(|c| c.is_ascii_digit())
        && !b.is_empty()
        && b.chars().all(|c| c.is_ascii_digit())
}

pub(crate) fn cache_has_stale_wallhaven_names(result: &FetchResult) -> bool {
    let Some(first) = result.items.first() else {
        return false;
    };
    if !first.url.contains("wallhaven") {
        return false;
    }
    result.items.iter().any(|i| {
        let n = i.name.trim();
        looks_like_resolution_caption(n)
            || ((5..=8).contains(&n.len())
                && n.chars().all(|c| c.is_ascii_alphanumeric())
                && !n.contains('×'))
    })
}

/// True when Bing Daily / NASA APOD rows lack preview-stats fields.
pub(crate) fn cache_has_incomplete_daily_meta(result: &FetchResult) -> bool {
    result.items.iter().any(|i| {
        let u = i.url.as_str();
        let daily = u.contains("bing.com")
            || u.contains("nasa.gov")
            || u.contains("apod.nasa.gov");
        if !daily {
            return false;
        }
        let missing_core = i.date.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_none()
            || i.page_url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_none()
            || i.file_type
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_none();
        if missing_core {
            return true;
        }
        // Unsplit Bing copyright used as Credit: "Scene… (© Photographer)".
        u.contains("bing.com")
            && i.credit.as_deref().is_some_and(|c| c.contains("(©"))
    })
}

pub(crate) fn write_search_cache(src: &CatalogSource, opts: &SearchOpts, result: &FetchResult) {
    let path = search_cache_file(src, opts);
    if let Ok(text) = serde_json::to_string(result) {
        let _ = std::fs::write(path, text);
    }
}

pub(crate) fn github_repos_cache_key(src: &CatalogSource) -> String {
    src.github_targets()
        .into_iter()
        .map(|(r, p)| format!("{r}:{p}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Parse Settings “Add”: `github.com/owner/repo[/path]` or a catalog JSON URL.
pub fn parse_added_source(raw: &str) -> Option<CatalogSource> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw
        .strip_prefix("https://github.com/")
        .or_else(|| raw.strip_prefix("http://github.com/"))
    {
        let rest = rest.trim_end_matches('/');
        let mut parts = rest.split('/');
        let owner = parts.next().unwrap_or("");
        let repo = parts.next().unwrap_or("");
        if owner.is_empty() || repo.is_empty() {
            return None;
        }
        let path = parts.collect::<Vec<_>>().join("/");
        return Some(CatalogSource {
            name: format!("{owner}/{repo}"),
            kind: "github".into(),
            url: String::new(),
            repo: format!("{owner}/{repo}"),
            path,
            repos: Vec::new(),
            api_key: String::new(),
        });
    }
    if raw.contains("github.com/") {
        return parse_added_source(&format!("https://{raw}"));
    }
    if !(raw.starts_with("https://") || raw.starts_with("http://")) {
        return None;
    }
    let name = raw
        .rsplit('/')
        .next()
        .unwrap_or("catalog")
        .trim_end_matches(".json");
    Some(CatalogSource {
        name: name.into(),
        kind: "index".into(),
        url: raw.into(),
        repo: String::new(),
        path: String::new(),
        repos: Vec::new(),
        api_key: String::new(),
    })
}

pub(crate) fn last_page_from_count(total: u32, per_page: u32) -> Option<u32> {
    if per_page == 0 {
        return Some(1);
    }
    if total == 0 {
        return Some(1);
    }
    Some(total.saturating_add(per_page - 1) / per_page)
}

