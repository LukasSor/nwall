use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use super::*;

pub(crate) const NASA_DAYS: u32 = 10;

pub(crate) fn fetch_nasa(api_key: &str) -> Result<Vec<RemoteItem>> {
    let key = if api_key.trim().is_empty() {
        std::env::var("NASA_API_KEY").unwrap_or_else(|_| "DEMO_KEY".into())
    } else {
        api_key.trim().to_string()
    };
    let has_key = !key.is_empty() && !key.eq_ignore_ascii_case("DEMO_KEY");
    note_source_auth("nasa", has_key);
    nasa_by_recent_dates(&key, has_key, NASA_DAYS)
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
        let file_type = nasa_file_type_from_url(&url)
            .filter(|t| matches!(t.as_str(), "MP4" | "WebM" | "MOV" | "MKV" | "AVI" | "M4V"))?;
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
        u.split(['?', '&']).skip(1).find_map(|p| {
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

pub(crate) fn nasa_by_recent_dates(key: &str, has_key: bool, days: u32) -> Result<Vec<RemoteItem>> {
    let days = days.max(1);
    let start = utc_ymd_days_ago(days.saturating_sub(1));
    let end = utc_ymd_days_ago(0);
    let url = format!(
        "https://api.nasa.gov/planetary/apod?api_key={}&start_date={}&end_date={}&thumbs=true",
        urlencoding_lite(key),
        start,
        end
    );
    match nasa_parse_listing(&url, has_key) {
        Ok(items) if !items.is_empty() => return Ok(items),
        Ok(_) => {}
        Err(e) if http_err_is_rate_limit(&e) => {
            return Err(e);
        }
        Err(e) => log::warn!("NASA APOD range failed: {e:#}"),
    }
    match nasa_one_date(key, has_key, &end) {
        Ok(Some(it)) => Ok(vec![it]),
        Ok(None) => Err(anyhow!("No results found")),
        Err(e) => Err(e),
    }
}

fn nasa_parse_listing(url: &str, has_key: bool) -> Result<Vec<RemoteItem>> {
    let raw: serde_json::Value = limited_get(url, "nasa", RequestClass::Listing, has_key)?
        .into_json()
        .context("NASA json")?;
    let mut items = Vec::new();
    if let Some(arr) = raw.as_array() {
        for v in arr {
            let a: NasaApod = serde_json::from_value(v.clone()).unwrap_or_default();
            if let Some(it) = nasa_item(a) {
                items.push(it);
            }
        }
    } else {
        let a: NasaApod = serde_json::from_value(raw).unwrap_or_default();
        if let Some(it) = nasa_item(a) {
            items.push(it);
        }
    }
    items.reverse();
    if items.is_empty() {
        Err(anyhow!("No results found"))
    } else {
        Ok(items)
    }
}

pub(crate) fn nasa_one_date(key: &str, has_key: bool, date: &str) -> Result<Option<RemoteItem>> {
    let url = format!(
        "https://api.nasa.gov/planetary/apod?api_key={}&date={}&thumbs=true",
        urlencoding_lite(key),
        date
    );
    let parsed: NasaApod = limited_get(&url, "nasa", RequestClass::Listing, has_key)
        .with_context(|| "NASA APOD")?
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
