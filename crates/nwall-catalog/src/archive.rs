use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use super::types::UA;
use super::*;

pub(crate) const ARCHIVE_ROWS: u32 = 12;
pub(crate) const ARCHIVE_HTTP_TIMEOUT_SECS: u64 = 10;
pub(crate) const ARCHIVE_MAX_FILE_BYTES: u64 = 80_000_000;

pub(crate) fn archive_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(UA)
        .timeout(std::time::Duration::from_secs(ARCHIVE_HTTP_TIMEOUT_SECS))
        .build()
}

pub(crate) fn fetch_archive(opts: &SearchOpts) -> Result<FetchResult> {
    let page = opts.page();
    let media = opts.media.trim().to_ascii_lowercase();
    let q = if opts.query.trim().is_empty() {
        match media.as_str() {
            "video" | "videos" => "live wallpaper",
            _ => "wallpaper",
        }
    } else {
        opts.query.trim()
    };
    let movies = r#"(mediatype:(movies) AND format:(MPEG4 OR "512Kb MPEG4" OR WebM OR "h.264") AND item_size:[0 TO 120000000])"#;
    let images = r#"(mediatype:(image) AND format:(JPEG OR PNG OR GIF OR WebP OR "Animated GIF") AND item_size:[0 TO 40000000])"#;
    let media_q = match media.as_str() {
        "image" | "images" => images,
        "video" | "videos" => movies,
        _ => "",
    };
    let solr = if media_q.is_empty() {
        format!("({q}) AND ({movies} OR {images})")
    } else {
        format!("({q}) AND {media_q}")
    };
    let mut url = format!(
        "https://archive.org/advancedsearch.php?q={}&fl[]=identifier&fl[]=title&fl[]=mediatype&fl[]=creator&fl[]=date&fl[]=year&fl[]=downloads&fl[]=item_size&fl[]=subject&fl[]=licenseurl&rows={}&page={}&output=json",
        urlencoding_lite(&solr),
        ARCHIVE_ROWS,
        page
    );
    match opts.order.as_str() {
        "latest" | "date" => url.push_str("&sort[]=publicdate+desc"),
        "popular" | "downloads" => url.push_str("&sort[]=downloads+desc"),
        _ => {}
    }
    #[derive(Deserialize)]
    struct IaSearch {
        response: IaResponse,
    }
    #[derive(Deserialize)]
    struct IaResponse {
        #[serde(default, rename = "numFound")]
        num_found: Option<u64>,
        #[serde(default)]
        docs: Vec<IaDoc>,
    }
    #[derive(Deserialize)]
    struct IaDoc {
        #[serde(default)]
        identifier: String,
        title: Option<String>,
        #[serde(default)]
        mediatype: Option<String>,
        #[serde(default)]
        creator: Option<Value>,
        #[serde(default)]
        date: Option<Value>,
        #[serde(default)]
        year: Option<Value>,
        #[serde(default)]
        downloads: Option<Value>,
        #[serde(default)]
        item_size: Option<Value>,
        #[serde(default)]
        subject: Option<Value>,
        #[serde(default)]
        licenseurl: Option<Value>,
    }
    let parsed: IaSearch = limited_get(&url, "archive", RequestClass::Listing, false)
        .context("archive.org search")?
        .into_json()
        .context("archive.org json")?;
    let n_docs = parsed.response.docs.len() as u32;
    let last_page = parsed
        .response
        .num_found
        .and_then(|t| {
            if t == 0 {
                Some(1)
            } else {
                last_page_from_count(t.min(u64::from(u32::MAX)) as u32, ARCHIVE_ROWS)
            }
        })
        .or_else(|| {
            if n_docs < ARCHIVE_ROWS {
                Some(page)
            } else {
                None
            }
        });
    let items = parsed
        .response
        .docs
        .into_iter()
        .filter(|d| !d.identifier.is_empty())
        .take(ARCHIVE_ROWS as usize)
        .map(|doc| {
            let mediatype = doc
                .mediatype
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let video = mediatype.as_deref() != Some("image");
            let mut item = RemoteItem {
                name: doc
                    .title
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| doc.identifier.clone()),
                video,
                url: format!("https://archive.org/download/{}", doc.identifier),
                thumb: Some(format!(
                    "https://archive.org/services/img/{}",
                    doc.identifier
                )),
                credit: Some("Internet Archive".into()),
                id: Some(doc.identifier.clone()),
                category: mediatype,
                uploader: json_stringish(doc.creator.as_ref()),
                date: json_stringish(doc.date.as_ref())
                    .or_else(|| json_stringish(doc.year.as_ref())),
                // IA field `downloads` == engagement / site "Views" (not file-only downloads).
                views: json_stringish(doc.downloads.as_ref())
                    .and_then(|s| s.parse().ok())
                    .or_else(|| doc.downloads.as_ref().and_then(|v| v.as_u64())),
                file_size: json_stringish(doc.item_size.as_ref())
                    .and_then(|s| s.parse().ok())
                    .or_else(|| doc.item_size.as_ref().and_then(|v| v.as_u64())),
                tags: archive_subjects_from_value(doc.subject.as_ref()),
                license: json_stringish(doc.licenseurl.as_ref())
                    .and_then(|s| archive_license_label(&s)),
                page_url: Some(format!("https://archive.org/details/{}", doc.identifier)),
                ..Default::default()
            };
            if let Some(d) = cached_archive_details(&doc.identifier) {
                item.apply_archive_details(&d);
            }
            item
        })
        .collect();
    Ok(FetchResult {
        items,
        page,
        last_page,
    })
}

/// Resolve deferred remote URLs (Archive.org item → concrete file) before play/download.
pub fn resolve_remote_url(url: &str) -> Result<String> {
    resolve_download_url(url)
}

pub(crate) fn archive_pick_file_url(identifier: &str) -> Option<String> {
    if let Some(name) = cached_archive_file_name(identifier) {
        return Some(format!(
            "https://archive.org/download/{identifier}/{}",
            percent_encode_path(&name)
        ));
    }
    let meta_url = format!("https://archive.org/metadata/{identifier}/files");
    let resp = match limited_get(&meta_url, "archive", RequestClass::Detail, false) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("archive.org files {identifier}: {e}");
            return None;
        }
    };
    let raw: Value = match resp.into_json() {
        Ok(m) => m,
        Err(e) => {
            log::warn!("archive.org files json {identifier}: {e}");
            return None;
        }
    };
    let files = raw
        .get("result")
        .and_then(|v| v.as_array())
        .or_else(|| raw.get("files").and_then(|v| v.as_array()))
        .cloned()
        .unwrap_or_default();
    let file = pick_archive_file_from_json(&files)?;
    let name = json_stringish(file.get("name"))?;
    Some(format!(
        "https://archive.org/download/{identifier}/{}",
        percent_encode_path(&name)
    ))
}

pub(crate) fn cached_archive_file_name(identifier: &str) -> Option<String> {
    let path = archive_detail_path(identifier);
    let text = std::fs::read_to_string(path).ok()?;
    let raw: Value = serde_json::from_str(&text).ok()?;
    let _ = raw;
    None
}

pub(crate) fn pick_archive_file_from_json(files: &[Value]) -> Option<Value> {
    let mut videos = Vec::new();
    let mut images = Vec::new();
    for f in files {
        let name = json_stringish(f.get("name")).unwrap_or_default();
        let n = name.to_ascii_lowercase();
        if n.is_empty() || archive_skip_file_name(&n) {
            continue;
        }
        let sz = json_stringish(f.get("size"))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        if n.ends_with(".mp4") || n.ends_with(".webm") {
            if sz == 0 || sz < ARCHIVE_MAX_FILE_BYTES {
                videos.push((sz, f.clone()));
            }
        } else if n.ends_with(".jpg")
            || n.ends_with(".jpeg")
            || n.ends_with(".png")
            || n.ends_with(".gif")
            || n.ends_with(".webp")
        {
            if sz == 0 || sz < ARCHIVE_MAX_FILE_BYTES {
                images.push((sz, f.clone()));
            }
        }
    }
    videos.sort_by_key(|(sz, _)| if *sz == 0 { u64::MAX } else { *sz });
    if let Some((_, f)) = videos.into_iter().next() {
        return Some(f);
    }
    images.sort_by_key(|(sz, _)| std::cmp::Reverse(*sz));
    images.into_iter().next().map(|(_, f)| f)
}

pub(crate) fn archive_skip_file_name(n: &str) -> bool {
    n.contains("_thumb")
        || n.contains("__ia")
        || n.contains("_spectrogram")
        || n.ends_with(".xml")
        || n.ends_with(".json")
        || n.contains("_files.xml")
}

pub(crate) fn percent_encode_path(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            let mut out = String::new();
            for b in seg.bytes() {
                match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        out.push(b as char);
                    }
                    _ => out.push_str(&format!("%{b:02X}")),
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("/")
}
