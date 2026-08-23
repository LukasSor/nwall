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

pub(crate) fn fetch_bing() -> Result<Vec<RemoteItem>> {
    // Two idx windows in parallel → up to 16 days at ~1× network RTT (API caps n=8).
    let http = listing_agent();
    let (a, b) = std::thread::scope(|scope| {
        let http_a = http.clone();
        let http_b = http.clone();
        let ha = scope.spawn(move || bing_page(&http_a, 0));
        let hb = scope.spawn(move || bing_page(&http_b, 8));
        (ha.join().unwrap(), hb.join().unwrap())
    });
    let mut images = a.context("bing page 0")?;
    match b {
        Ok(more) => images.extend(more),
        Err(e) => log::warn!("bing page idx=8: {e:#}"),
    }
    let mut seen = std::collections::HashSet::new();
    Ok(images
        .into_iter()
        .filter_map(bing_item)
        .filter(|it| seen.insert(it.url.clone()))
        .collect())
}

fn bing_page(http: &ureq::Agent, idx: u32) -> Result<Vec<BingImage>> {
    let url = format!(
        "https://www.bing.com/HPImageArchive.aspx?format=js&idx={idx}&n=8&mkt=en-US"
    );
    let parsed: BingArchivePage = http
        .get(&url)
        .call()
        .with_context(|| format!("bing HPImageArchive idx={idx}"))?
        .into_json()
        .context("bing json")?;
    Ok(parsed.images)
}

#[derive(Deserialize)]
struct BingArchivePage {
    #[serde(default)]
    images: Vec<BingImage>,
}

#[derive(Default, Deserialize)]
pub(crate) struct BingImage {
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) copyright: String,
    #[serde(default)]
    pub(crate) copyrightlink: String,
    #[serde(default)]
    pub(crate) startdate: String,
    #[serde(default)]
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) urlbase: String,
}

pub(crate) fn bing_item(im: BingImage) -> Option<RemoteItem> {
    let rel = if !im.urlbase.is_empty() {
        format!("{}_UHD.jpg", im.urlbase)
    } else {
        im.url.clone()
    };
    if rel.is_empty() {
        return None;
    }
    let url = bing_abs_url(&rel)?;
    let thumb = if im.urlbase.is_empty() {
        None
    } else {
        Some(format!("https://www.bing.com{}_400x240.jpg", im.urlbase))
    };

    let title = im.title.trim();
    let (caption, credit_body) = bing_split_copyright(im.copyright.trim());
    let name = if !title.is_empty() {
        title.to_string()
    } else if let Some(ref c) = caption {
        c.clone()
    } else if !credit_body.is_empty() {
        credit_body.clone()
    } else {
        "Bing Daily".into()
    };

    let credit = if credit_body.is_empty() {
        Some("Bing".into())
    } else {
        Some(credit_body)
    };

    // Caption only when Title is the poetic headline (otherwise caption is the tile name).
    let description = if !title.is_empty() { caption } else { None };

    Some(RemoteItem {
        name,
        video: false,
        url,
        thumb,
        credit,
        id: bing_id_from_urlbase(&im.urlbase),
        date: bing_format_startdate(&im.startdate),
        description,
        page_url: bing_abs_url(&im.copyrightlink),
        file_type: Some("JPEG".into()),
        ..Default::default()
    })
}

/// `20260821` → `2026-08-21`.
pub(crate) fn bing_format_startdate(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        return Some(format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]));
    }
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// `"Scene, Place (© Photographer/Agency)"` → caption + credit.
pub(crate) fn bing_split_copyright(copyright: &str) -> (Option<String>, String) {
    let c = copyright.trim();
    if c.is_empty() {
        return (None, String::new());
    }
    let Some(i) = c.find("(©") else {
        return (None, c.to_string());
    };
    let caption = c[..i].trim().trim_end_matches(',').trim();
    let credit = c[i..]
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    let caption = if caption.is_empty() {
        None
    } else {
        Some(caption.to_string())
    };
    let credit = if credit.is_empty() {
        c.to_string()
    } else {
        credit.to_string()
    };
    (caption, credit)
}

pub(crate) fn bing_id_from_urlbase(urlbase: &str) -> Option<String> {
    let raw = urlbase.trim();
    if raw.is_empty() {
        return None;
    }
    let id = raw
        .split("id=")
        .nth(1)
        .map(|s| s.split('&').next().unwrap_or(s).trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    id.or_else(|| {
        let leaf = raw.rsplit('/').next().unwrap_or(raw).trim();
        if leaf.is_empty() {
            None
        } else {
            Some(leaf.to_string())
        }
    })
}

pub(crate) fn bing_abs_url(rel_or_abs: &str) -> Option<String> {
    let s = rel_or_abs.trim();
    if s.is_empty() {
        return None;
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        Some(s.to_string())
    } else if s.starts_with('/') {
        Some(format!("https://www.bing.com{s}"))
    } else {
        Some(format!("https://www.bing.com/{s}"))
    }
}
