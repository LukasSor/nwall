
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use super::*;

pub(crate) fn pixabay_tags_name(tags: &str, id: u64) -> (Vec<String>, String) {
    let tags: Vec<String> = tags
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let name = tags
        .first()
        .cloned()
        .unwrap_or_else(|| format!("pixabay-{id}"));
    (tags, name)
}

pub(crate) fn pixabay_uploader(user: &str) -> Option<String> {
    let u = user.trim();
    if u.is_empty() {
        None
    } else {
        Some(u.to_string())
    }
}

pub(crate) fn pixabay_opt_url(url: &str) -> Option<String> {
    let u = url.trim();
    if u.is_empty() {
        None
    } else {
        Some(u.to_string())
    }
}

pub(crate) fn pixabay_type_category(ty: &str) -> Option<String> {
    let t = ty.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

pub(crate) fn fetch_pixabay_photos(
    key: &str,
    q: &str,
    page: u32,
    per_page: u32,
    order: &str,
    safesearch: bool,
    category: &str,
) -> Result<(Vec<RemoteItem>, Option<u32>)> {
    let mut url = format!(
        "https://pixabay.com/api/?key={}&q={}&image_type=photo&orientation=horizontal&safesearch={}&per_page={}&page={}&order={}",
        urlencoding_lite(key),
        urlencoding_lite(q),
        if safesearch { "true" } else { "false" },
        per_page,
        page,
        order
    );
    if !category.is_empty() {
        url.push_str(&format!("&category={}", urlencoding_lite(category)));
    }
    #[derive(Deserialize)]
    struct PixabayPhotos {
        #[serde(default, rename = "totalHits")]
        total_hits: u32,
        #[serde(default)]
        hits: Vec<PixabayPhotoHit>,
    }
    #[derive(Deserialize)]
    struct PixabayPhotoHit {
        #[serde(default)]
        id: u64,
        #[serde(default)]
        tags: String,
        #[serde(default, rename = "type")]
        media_type: String,
        #[serde(default)]
        user: String,
        #[serde(default, rename = "userImageURL")]
        user_image_url: String,
        #[serde(default, rename = "previewURL")]
        preview_url: String,
        #[serde(default, rename = "webformatURL")]
        webformat_url: String,
        #[serde(default, rename = "largeImageURL")]
        large_image_url: String,
        #[serde(default, rename = "imageWidth")]
        image_width: Option<u32>,
        #[serde(default, rename = "imageHeight")]
        image_height: Option<u32>,
        #[serde(default, rename = "imageSize")]
        image_size: Option<u64>,
        #[serde(default)]
        downloads: Option<u64>,
        #[serde(default)]
        views: Option<u64>,
        #[serde(default)]
        likes: Option<u64>,
        #[serde(default)]
        comments: Option<u64>,
        #[serde(default, rename = "pageURL")]
        page_url: String,
    }
    let parsed: PixabayPhotos = listing_agent()
        .get(&url)
        .call()
        .context("pixabay photos")?
        .into_json()
        .context("pixabay photos json")?;
    let last_page = last_page_from_count(parsed.total_hits, per_page);
    let items = parsed
        .hits
        .into_iter()
        .filter_map(|h| {
            let url = [&h.large_image_url, &h.webformat_url, &h.preview_url]
                .into_iter()
                .find(|u| !u.is_empty())?
                .clone();
            let (tags, name) = pixabay_tags_name(&h.tags, h.id);
            let thumb = if !h.preview_url.is_empty() {
                Some(h.preview_url)
            } else if !h.webformat_url.is_empty() {
                Some(h.webformat_url)
            } else {
                None
            };
            let ext = ext_from_url(&url).to_ascii_lowercase();
            let file_type = if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif") {
                Some(ext)
            } else {
                Some("jpg".into())
            };
            Some(RemoteItem {
                name,
                video: false,
                url,
                thumb,
                credit: Some("Pixabay".into()),
                id: Some(h.id.to_string()),
                width: h.image_width.filter(|n| *n > 0),
                height: h.image_height.filter(|n| *n > 0),
                file_size: h.image_size.filter(|n| *n > 0),
                category: pixabay_type_category(&h.media_type),
                tags,
                uploader: pixabay_uploader(&h.user),
                avatar: pixabay_opt_url(&h.user_image_url),
                downloads: h.downloads.filter(|n| *n > 0),
                views: h.views.filter(|n| *n > 0),
                favorites: h.likes.filter(|n| *n > 0),
                comments: h.comments.filter(|n| *n > 0),
                page_url: pixabay_opt_url(&h.page_url),
                file_type,
                ..Default::default()
            })
        })
        .collect();
    Ok((items, last_page))
}

pub(crate) fn fetch_pixabay_videos(
    key: &str,
    q: &str,
    page: u32,
    per_page: u32,
    order: &str,
    safesearch: bool,
    category: &str,
    video_type: &str,
) -> Result<(Vec<RemoteItem>, Option<u32>)> {
    let mut url = format!(
        "https://pixabay.com/api/videos/?key={}&q={}&orientation=horizontal&safesearch={}&per_page={}&page={}&order={}&video_type={}",
        urlencoding_lite(key),
        urlencoding_lite(q),
        if safesearch { "true" } else { "false" },
        per_page,
        page,
        order,
        video_type
    );
    if !category.is_empty() {
        url.push_str(&format!("&category={}", urlencoding_lite(category)));
    }
    #[derive(Deserialize)]
    struct PixabayVideos {
        #[serde(default, rename = "totalHits")]
        total_hits: u32,
        #[serde(default)]
        hits: Vec<PixabayHit>,
    }
    #[derive(Deserialize)]
    struct PixabayHit {
        #[serde(default)]
        id: u64,
        #[serde(default)]
        tags: String,
        #[serde(default, rename = "type")]
        media_type: String,
        #[serde(default)]
        picture_id: String,
        #[serde(default)]
        duration: Option<u32>,
        #[serde(default)]
        user: String,
        #[serde(default, rename = "userImageURL")]
        user_image_url: String,
        #[serde(default)]
        downloads: Option<u64>,
        #[serde(default)]
        views: Option<u64>,
        #[serde(default)]
        likes: Option<u64>,
        #[serde(default)]
        comments: Option<u64>,
        #[serde(default, rename = "pageURL")]
        page_url: String,
        videos: Option<PixabayVideoSet>,
    }
    #[derive(Deserialize)]
    struct PixabayVideoSet {
        large: Option<PixabayFile>,
        medium: Option<PixabayFile>,
        small: Option<PixabayFile>,
        tiny: Option<PixabayFile>,
    }
    #[derive(Deserialize)]
    struct PixabayFile {
        #[serde(default)]
        url: String,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
        #[serde(default)]
        size: Option<u64>,
        /// Prefer cdn.pixabay.com still preview over vimeocdn.
        #[serde(default)]
        thumbnail: String,
    }
    let parsed: PixabayVideos = listing_agent()
        .get(&url)
        .call()
        .context("pixabay videos")?
        .into_json()
        .context("pixabay videos json")?;
    let last_page = last_page_from_count(parsed.total_hits, per_page);
    let items = parsed
        .hits
        .into_iter()
        .filter_map(|h| {
            let vids = h.videos?;
            // Prefer medium/small; large as fallback.
            let file = vids
                .medium
                .as_ref()
                .or(vids.small.as_ref())
                .or(vids.tiny.as_ref())
                .or(vids.large.as_ref())
                .filter(|f| !f.url.is_empty())?;
            let (tags, name) = pixabay_tags_name(&h.tags, h.id);
            // Prefer the smallest still for Discover tiles (scaled further in the GUI).
            let thumb = [
                vids.tiny.as_ref().map(|f| f.thumbnail.as_str()).unwrap_or(""),
                vids.small.as_ref().map(|f| f.thumbnail.as_str()).unwrap_or(""),
                file.thumbnail.as_str(),
                vids.medium.as_ref().map(|f| f.thumbnail.as_str()).unwrap_or(""),
                vids.large.as_ref().map(|f| f.thumbnail.as_str()).unwrap_or(""),
            ]
            .into_iter()
            .find(|u| !u.is_empty())
            .map(|u| u.to_string())
            .or_else(|| {
                let id = h.picture_id.trim();
                if id.is_empty() {
                    None
                } else {
                    Some(format!("https://i.vimeocdn.com/video/{id}_295x166.jpg"))
                }
            });
            let (width, height) = [
                vids.large.as_ref(),
                vids.medium.as_ref(),
                vids.small.as_ref(),
                vids.tiny.as_ref(),
            ]
            .into_iter()
            .flatten()
            .find_map(|f| match (f.width.filter(|n| *n > 0), f.height.filter(|n| *n > 0)) {
                (Some(w), Some(h)) => Some((w, h)),
                _ => None,
            })
            .map(|(w, h)| (Some(w), Some(h)))
            .unwrap_or((
                file.width.filter(|n| *n > 0),
                file.height.filter(|n| *n > 0),
            ));
            Some(RemoteItem {
                name,
                video: true,
                url: file.url.clone(),
                thumb,
                credit: Some("Pixabay".into()),
                id: Some(h.id.to_string()),
                width,
                height,
                file_size: file.size.filter(|n| *n > 0),
                category: pixabay_type_category(&h.media_type),
                tags,
                duration_secs: h.duration.filter(|d| *d > 0).map(|d| f64::from(d)),
                uploader: pixabay_uploader(&h.user),
                avatar: pixabay_opt_url(&h.user_image_url),
                downloads: h.downloads.filter(|n| *n > 0),
                views: h.views.filter(|n| *n > 0),
                favorites: h.likes.filter(|n| *n > 0),
                comments: h.comments.filter(|n| *n > 0),
                page_url: pixabay_opt_url(&h.page_url),
                file_type: Some("mp4".into()),
                ..Default::default()
            })
        })
        .collect();
    Ok((items, last_page))
}

pub(crate) fn interleave_remote_items(a: Vec<RemoteItem>, b: Vec<RemoteItem>) -> Vec<RemoteItem> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let mut ai = a.into_iter();
    let mut bi = b.into_iter();
    loop {
        match (ai.next(), bi.next()) {
            (Some(x), Some(y)) => {
                out.push(x);
                out.push(y);
            }
            (Some(x), None) => {
                out.push(x);
                out.extend(ai);
                break;
            }
            (None, Some(y)) => {
                out.push(y);
                out.extend(bi);
                break;
            }
            (None, None) => break,
        }
    }
    out
}

pub(crate) fn fetch_pixabay(opts: &SearchOpts, api_key: &str) -> Result<FetchResult> {
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return Err(anyhow!(
            "Pixabay needs a free API key — set PIXABAY_API_KEY or add the source as “pixabay YOUR_KEY” (pixabay.com/api/docs/)"
        ));
    }
    let page = opts.page();
    let q = if opts.query.trim().is_empty() {
        "nature"
    } else {
        opts.query.trim()
    };
    let order = match opts.order.as_str() {
        "latest" => "latest",
        _ => "popular",
    };
    let video_type = match opts.video_type.as_str() {
        "film" | "animation" => opts.video_type.as_str(),
        _ => "all",
    };
    let category = opts.category.trim();
    let media = opts.media.trim().to_ascii_lowercase();
    match media.as_str() {
        "image" | "images" => {
            let (items, last_page) =
                fetch_pixabay_photos(&key, q, page, 20, order, opts.safesearch, category)?;
            Ok(FetchResult {
                items,
                page,
                last_page,
            })
        }
        "video" | "videos" => {
            let (items, last_page) = fetch_pixabay_videos(
                &key,
                q,
                page,
                20,
                order,
                opts.safesearch,
                category,
                video_type,
            )?;
            Ok(FetchResult {
                items,
                page,
                last_page,
            })
        }
        _ => {
            // Mix ~10 photos + ~10 videos so All stays one balanced page.
            let (photos, photo_last) =
                fetch_pixabay_photos(&key, q, page, 10, order, opts.safesearch, category)?;
            let (videos, video_last) = fetch_pixabay_videos(
                &key,
                q,
                page,
                10,
                order,
                opts.safesearch,
                category,
                video_type,
            )?;
            let last_page = match (photo_last, video_last) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            Ok(FetchResult {
                items: interleave_remote_items(photos, videos),
                page,
                last_page,
            })
        }
    }
}

