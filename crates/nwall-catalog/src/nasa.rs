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

pub(crate) const NASA_DAYS: u32 = 10;
pub(crate) const NASA_FETCH_CONCURRENCY: usize = 3;

pub(crate) fn fetch_nasa(api_key: &str) -> Result<Vec<RemoteItem>> {
    let key = if api_key.trim().is_empty() {
        std::env::var("NASA_API_KEY").unwrap_or_else(|_| "DEMO_KEY".into())
    } else {
        api_key.trim().to_string()
    };
    let http = listing_agent();

    nasa_by_recent_dates(&http, &key, NASA_DAYS)
}

pub(crate) fn nasa_item(a: NasaApod) -> Option<RemoteItem> {
    let media = a.media_type.trim().to_ascii_lowercase();
    let url = a.url.trim().to_string();
    if url.is_empty() {
        return None;
    }

    let date = a
        .date
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let page_url = date.as_deref().and_then(nasa_apod_page_url);
    let credit = a
        .copyright
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|c| format!("NASA / {c}"))
        .unwrap_or_else(|| "NASA APOD".into());
    let description = a
        .explanation
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let name = if a.title.trim().is_empty() {
        "NASA APOD".into()
    } else {
        a.title.trim().to_string()
    };

    if media == "image" {
        let full = a
            .hdurl
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| url.clone());
        return Some(RemoteItem {
            name,
            video: false,
            // HD only on download/preview; thumb stays the lighter `url`.
            url: full.clone(),
            thumb: Some(url),
            credit: Some(credit),
            id: date.clone(),
            date,
            description,
            page_url,
            file_type: nasa_file_type_from_url(&full),
            ..Default::default()
        });
    }

    if media == "video" {
        let file_type = nasa_file_type_from_url(&url).filter(|t| {
            matches!(
                t.as_str(),
                "MP4" | "WebM" | "MOV" | "MKV" | "AVI" | "M4V"
            )
        })?;
        let thumb = a
            .thumbnail_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        return Some(RemoteItem {
            name,
            video: true,
            url,
            thumb,
            credit: Some(credit),
            id: date.clone(),
            date,
            description,
            page_url: page_url.or_else(|| nasa_youtube_watch_url(&a.url)),
            file_type: Some(file_type),
            ..Default::default()
        });
    }

    None
}

#[derive(Default, Deserialize)]
pub(crate) struct NasaApod {
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) hdurl: Option<String>,
    #[serde(default)]
    pub(crate) media_type: String,
    #[serde(default)]
    pub(crate) copyright: Option<String>,
    #[serde(default)]
    pub(crate) date: Option<String>,
    #[serde(default)]
    pub(crate) explanation: Option<String>,
    /// Present when `thumbs=true` and `media_type=video`.
    #[serde(default)]
    pub(crate) thumbnail_url: Option<String>,
}

/// `YYYY-MM-DD` → `https://apod.nasa.gov/apod/apYYMMDD.html`.
pub(crate) fn nasa_apod_page_url(date: &str) -> Option<String> {
    let d = date.trim();
    let b = d.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y = &d[2..4];
    let m = &d[5..7];
    let day = &d[8..10];
    if !(y.bytes().chain(m.bytes()).chain(day.bytes())).all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("https://apod.nasa.gov/apod/ap{y}{m}{day}.html"))
}

/// YouTube embed / watch / youtu.be → canonical watch URL (for video APODs).
pub(crate) fn nasa_youtube_watch_url(url: &str) -> Option<String> {
    let u = url.trim();
    if u.is_empty() {
        return None;
    }
    let lower = u.to_ascii_lowercase();
    let id = if let Some(idx) = lower.find("youtube.com/embed/") {
        let start = idx + "youtube.com/embed/".len();
        u.get(start..)?.split(['?', '&', '/']).next()
    } else if let Some(idx) = lower.find("youtube-nocookie.com/embed/") {
        let start = idx + "youtube-nocookie.com/embed/".len();
        u.get(start..)?.split(['?', '&', '/']).next()
    } else if let Some(idx) = lower.find("youtu.be/") {
        let start = idx + "youtu.be/".len();
        u.get(start..)?.split(['?', '&', '/']).next()
    } else if lower.contains("youtube.com/watch") {
        u.split(['?', '&'])
            .skip(1)
            .find_map(|p| {
                let (k, v) = p.split_once('=')?;
                k.eq_ignore_ascii_case("v").then_some(v)
            })
    } else {
        None
    }?;
    let id = id.trim();
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    Some(format!("https://www.youtube.com/watch?v={id}"))
}

pub(crate) fn nasa_file_type_from_url(url: &str) -> Option<String> {
    let path = url.split('?').next().unwrap_or(url).trim();
    if path.is_empty() {
        return None;
    }
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("JPEG".into()),
        "png" => Some("PNG".into()),
        "gif" => Some("GIF".into()),
        "webp" => Some("WebP".into()),
        "bmp" => Some("BMP".into()),
        "mp4" | "m4v" => Some("MP4".into()),
        "webm" => Some("WebM".into()),
        other if !other.is_empty() && other.len() <= 8 => Some(other.to_ascii_uppercase()),
        _ => None,
    }
}

pub(crate) fn nasa_by_recent_dates(http: &ureq::Agent, key: &str, days: u32) -> Result<Vec<RemoteItem>> {
    let dates: Vec<String> = (0..days).map(utc_ymd_days_ago).collect();
    let mut items = Vec::new();
    let mut errors = Vec::new();
    let mut rate_limited = false;
    for chunk in dates.chunks(NASA_FETCH_CONCURRENCY) {
        if rate_limited {
            break;
        }
        let key = key.to_string();
        let batch: Vec<(String, Result<Option<RemoteItem>>)> = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(chunk.len());
            for date in chunk {
                let http = http.clone();
                let key = key.clone();
                let date = date.clone();
                handles.push(scope.spawn(move || {
                    let res = nasa_one_date(&http, &key, &date);
                    (date, res)
                }));
            }
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect()
        });
        for (date, res) in batch {
            match res {
                Ok(Some(it)) => items.push(it),
                Ok(None) => {}
                Err(e) => {
                    let msg = format!("{e:#}");
                    if msg.contains("429") || msg.to_ascii_lowercase().contains("rate limit") {
                        rate_limited = true;
                    }
                    log::warn!("NASA APOD {date}: {msg}");
                    errors.push(format!("{date}: {msg}"));
                }
            }
        }
        if items.len() >= 6 {
            break;
        }
    }
    if items.is_empty() {
        if rate_limited {
            return Err(anyhow!(
                "Rate limit reached — add a free API key next to NASA APOD in Settings, or try again later."
            ));
        }
        if errors.is_empty() {
            return Err(anyhow!("No results found"));
        }
        return Err(anyhow!("NASA APOD: {}", errors.join("; ")));
    }
    Ok(items)
}

pub(crate) fn nasa_one_date(http: &ureq::Agent, key: &str, date: &str) -> Result<Option<RemoteItem>> {
    let url = format!(
        "https://api.nasa.gov/planetary/apod?api_key={}&date={}",
        urlencoding_lite(key),
        date
    );
    let parsed: NasaApod = http
        .get(&url)
        .call()
        .with_context(|| format!("NASA APOD {date}"))?
        .into_json()
        .context("NASA json")?;
    Ok(nasa_item(parsed))
}

/// UTC calendar date `days_ago` days before today (`YYYY-MM-DD`).
pub(crate) fn utc_ymd_days_ago(days_ago: u32) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let day = (secs / 86_400).saturating_sub(u64::from(days_ago)) as i64;
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since Unix epoch → (year, month, day). Howard Hinnant’s algorithm.
pub(crate) fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

