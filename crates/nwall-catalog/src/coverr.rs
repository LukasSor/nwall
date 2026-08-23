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

pub(crate) fn fetch_coverr(opts: &SearchOpts, api_key: &str) -> Result<FetchResult> {
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return Err(anyhow!(
            "Coverr needs a free API key — add it next to Coverr in Settings (coverr.co/developers)"
        ));
    }
    let page = opts.page();
    let coverr_page = page.saturating_sub(1);
    let sort = match opts.order.as_str() {
        "date" | "latest" => "date",
        "trending" => "trending",
        _ => "popular",
    };
    let mut url = format!(
        "https://api.coverr.co/videos?api_key={}&urls=true&page_size=20&page={}&sort={}",
        urlencoding_lite(&key),
        coverr_page,
        sort
    );
    if !opts.query.trim().is_empty() {
        url.push_str(&format!("&query={}", urlencoding_lite(opts.query.trim())));
    }
    #[derive(Deserialize)]
    struct CoverrList {
        #[serde(default)]
        hits: Vec<CoverrHit>,
        pages: Option<u32>,
        total: Option<u32>,
    }
    #[derive(Deserialize)]
    struct CoverrHit {
        #[serde(default)]
        title: String,
        thumbnail: Option<String>,
        poster: Option<String>,
        #[serde(default)]
        is_vertical: bool,
        #[serde(default)]
        duration: Option<f64>,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
        urls: Option<CoverrUrls>,
    }
    #[derive(Deserialize)]
    struct CoverrUrls {
        mp4: Option<String>,
        mp4_download: Option<String>,
        mp4_preview: Option<String>,
    }
    let parsed: CoverrList = listing_agent()
        .get(&url)
        .call()
        .context("coverr videos")?
        .into_json()
        .context("coverr json")?;
    let n_hits = parsed.hits.len() as u32;
    let last_page = parsed
        .pages
        .filter(|p| *p > 0)
        .or_else(|| parsed.total.and_then(|t| last_page_from_count(t, 20)))
        .or_else(|| {
            if n_hits < 20 {
                Some(page)
            } else {
                None
            }
        });
    let items = parsed
        .hits
        .into_iter()
        .filter(|h| !h.is_vertical)
        .filter_map(|h| {
            let urls = h.urls?;
            let file = urls
                .mp4_download
                .or(urls.mp4)
                .or(urls.mp4_preview)
                .filter(|u| !u.is_empty())?;
            Some(RemoteItem {
                name: if h.title.is_empty() {
                    "Coverr".into()
                } else {
                    h.title
                },
                video: true,
                url: file,
                thumb: h.thumbnail.or(h.poster),
                credit: Some("Video from Coverr".into()),
                width: h.width.filter(|n| *n > 0),
                height: h.height.filter(|n| *n > 0),
                duration_secs: h.duration.filter(|d| d.is_finite() && *d > 0.0),
                file_type: Some("mp4".into()),
                ..Default::default()
            })
        })
        .collect();
    Ok(FetchResult {
        items,
        page,
        last_page,
    })
}

