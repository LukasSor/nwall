use serde_json::Value;

use super::client::{avatar_cache_get, avatar_cache_put, wait_yt_slot, yt_http, YT_PLAYER_TIMEOUT};
use super::ids::yt_video_id;
use crate::MusicTrack;

pub(super) fn remix_channel_avatar(item: &Value) -> Option<String> {
    for ptr in [
        "/channelThumbnailSupportedRenderers/channelThumbnailWithLinkRenderer/thumbnail/thumbnails",
        "/channelThumbnail/channelThumbnailWithLinkRenderer/thumbnail/thumbnails",
        "/channelThumbnailWithLinkRenderer/thumbnail/thumbnails",
        "/thumbnail/channelThumbnailWithLinkRenderer/thumbnail/thumbnails",
    ] {
        if let Some(url) = pick_channel_thumb(item.pointer(ptr), false) {
            return Some(url);
        }
    }
    find_channel_avatar(item)
}

pub(super) fn yt_json_avatar(raw: &Value) -> Option<String> {
    for ptr in [
        "/channel_thumbnail",
        "/uploader_avatar",
        "/channel/thumbnail",
        "/thumbnails",
    ] {
        if let Some(url) = pick_channel_thumb(raw.pointer(ptr), true) {
            return Some(url);
        }
    }
    find_channel_avatar(raw)
}

fn find_channel_avatar(v: &Value) -> Option<String> {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if k == "musicThumbnailRenderer"
                    || k == "musicItemThumbnailOverlayRenderer"
                    || k == "thumbnailOverlay"
                {
                    continue;
                }
                let channelish = k.contains("channelThumbnail")
                    || k == "videoOwnerRenderer"
                    || k == "avatarViewModel";
                if channelish {
                    if let Some(url) = first_avatar_in(val, false) {
                        return Some(url);
                    }
                }
                if let Some(url) = find_channel_avatar(val) {
                    return Some(url);
                }
            }
            None
        }
        Value::Array(arr) => {
            for val in arr {
                if let Some(url) = find_channel_avatar(val) {
                    return Some(url);
                }
            }
            None
        }
        _ => None,
    }
}

fn first_avatar_in(v: &Value, require_style: bool) -> Option<String> {
    if let Some(url) = pick_channel_thumb(Some(v), require_style) {
        return Some(url);
    }
    match v {
        Value::Object(map) => {
            for key in ["thumbnails", "sources", "thumbnail"] {
                if let Some(url) = pick_channel_thumb(map.get(key), require_style) {
                    return Some(url);
                }
            }
            for val in map.values() {
                if let Some(url) = first_avatar_in(val, require_style) {
                    return Some(url);
                }
            }
            None
        }
        Value::Array(arr) => pick_channel_thumb(Some(v), require_style).or_else(|| {
            arr.iter()
                .find_map(|val| first_avatar_in(val, require_style))
        }),
        _ => None,
    }
}

fn pick_channel_thumb(v: Option<&Value>, require_style: bool) -> Option<String> {
    let mut best: Option<(i64, String)> = None;
    visit_thumb_entries(v?, &mut |url, w, h| {
        if let Some(score) = avatar_thumb_score(url, w, h, require_style) {
            if best.as_ref().is_none_or(|(s, _)| score < *s) {
                best = Some((score, url.to_string()));
            }
        }
    });
    best.map(|(_, u)| u)
}

fn visit_thumb_entries(v: &Value, f: &mut impl FnMut(&str, i64, i64)) {
    match v {
        Value::Array(arr) => {
            for t in arr {
                visit_thumb_entries(t, f);
            }
        }
        Value::Object(map) => {
            if let Some(url) = map
                .get("url")
                .and_then(|x| x.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let w = map.get("width").and_then(|x| x.as_i64()).unwrap_or(0);
                let h = map.get("height").and_then(|x| x.as_i64()).unwrap_or(0);
                f(url, w, h);
            } else {
                for key in ["thumbnails", "sources"] {
                    if let Some(inner) = map.get(key) {
                        visit_thumb_entries(inner, f);
                    }
                }
            }
        }
        Value::String(s) => {
            let s = s.trim();
            if !s.is_empty() {
                f(s, 0, 0);
            }
        }
        _ => {}
    }
}

fn avatar_thumb_score(url: &str, width: i64, height: i64, require_style: bool) -> Option<i64> {
    if is_video_thumb_url(url) {
        return None;
    }
    let styled = looks_like_channel_avatar(url);
    if require_style && !styled {
        return None;
    }
    if !styled && !is_channel_cdn(url) {
        return None;
    }
    let w = if width > 0 {
        width
    } else {
        avatar_size_from_url(url).unwrap_or(88)
    };
    let h = if height > 0 { height } else { w };
    if w.min(h) < 24 || w.max(h) > 512 {
        return None;
    }
    let ratio = w.max(h) as f64 / w.min(h).max(1) as f64;
    if ratio > 1.2 {
        return None;
    }
    Some((w - 88).abs())
}

fn is_video_thumb_url(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.contains("i.ytimg.com/")
        || l.contains("i9.ytimg.com/")
        || l.contains("/mqdefault")
        || l.contains("/hqdefault")
        || l.contains("/sddefault")
        || l.contains("/maxresdefault")
        || l.contains("/vi/")
}

fn is_channel_cdn(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.contains("yt3.ggpht.com") || l.contains("yt3.googleusercontent.com")
}

fn looks_like_channel_avatar(url: &str) -> bool {
    if !is_channel_cdn(url) {
        return false;
    }
    let l = url.to_ascii_lowercase();
    l.contains("-c-k-")
        || l.contains("-no-rj")
        || l.contains("=s48-")
        || l.contains("=s72-")
        || l.contains("=s88-")
        || l.contains("=s100-")
        || l.contains("=s176-")
}

fn avatar_size_from_url(url: &str) -> Option<i64> {
    let l = url.to_ascii_lowercase();
    let idx = l.find("=s")?;
    let rest = &l[idx + 2..];
    let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    n.parse().ok().filter(|v| *v > 0)
}

/// Channel / uploader avatar URL (search JSON, then InnerTube `next`).
pub fn fetch_yt_music_avatar(track: &MusicTrack) -> Option<String> {
    if let Some(url) = track
        .avatar
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(url.to_string());
    }
    let id = yt_video_id(track).ok()?;
    if let Some(cached) = avatar_cache_get(&id) {
        return cached;
    }
    let url = innertube_owner_avatar(&id);
    avatar_cache_put(id, url.clone());
    url
}

fn innertube_owner_avatar(video_id: &str) -> Option<String> {
    wait_yt_slot();
    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "WEB",
                "clientVersion": "2.20240724.00.00",
                "hl": "en",
            }
        },
        "videoId": video_id,
    });
    let resp = yt_http()
        .post("https://www.youtube.com/youtubei/v1/next?prettyPrint=false")
        .set("Content-Type", "application/json")
        .set("Origin", "https://www.youtube.com")
        .set("Referer", "https://www.youtube.com/")
        .timeout(YT_PLAYER_TIMEOUT)
        .send_json(body)
        .ok()?;
    let raw: Value = resp.into_json().ok()?;
    find_owner_avatar(&raw)
}

fn find_owner_avatar(v: &Value) -> Option<String> {
    match v {
        Value::Object(map) => {
            if let Some(own) = map.get("videoOwnerRenderer") {
                if let Some(url) = pick_channel_thumb(own.pointer("/thumbnail/thumbnails"), false)
                    .or_else(|| first_avatar_in(own, false))
                {
                    return Some(url);
                }
            }
            if let Some(av) = map.get("avatarViewModel") {
                if let Some(url) = first_avatar_in(av, false) {
                    return Some(url);
                }
            }
            for val in map.values() {
                if let Some(url) = find_owner_avatar(val) {
                    return Some(url);
                }
            }
            None
        }
        Value::Array(arr) => arr.iter().find_map(find_owner_avatar),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_channel_thumb_skips_video_and_album_art() {
        let thumbs = serde_json::json!([
            {
                "url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg",
                "width": 320,
                "height": 180
            },
            {
                "url": "https://yt3.googleusercontent.com/album=w60-h60-l90-rj",
                "width": 60,
                "height": 60
            },
            {
                "url": "https://yt3.ggpht.com/face=s48-c-k-c0x00ffffff-no-rj",
                "width": 48,
                "height": 48
            },
            {
                "url": "https://yt3.ggpht.com/face=s88-c-k-c0x00ffffff-no-rj",
                "width": 88,
                "height": 88
            }
        ]);
        assert_eq!(
            pick_channel_thumb(Some(&thumbs), true).as_deref(),
            Some("https://yt3.ggpht.com/face=s88-c-k-c0x00ffffff-no-rj")
        );
    }

    #[test]
    fn owner_renderer_avatar() {
        let raw = serde_json::json!({
            "contents": {
                "videoSecondaryInfoRenderer": {
                    "owner": {
                        "videoOwnerRenderer": {
                            "thumbnail": {
                                "thumbnails": [
                                    {
                                        "url": "https://yt3.ggpht.com/x=s48-c-k-c0x00ffffff-no-rj",
                                        "width": 48,
                                        "height": 48
                                    },
                                    {
                                        "url": "https://yt3.ggpht.com/x=s88-c-k-c0x00ffffff-no-rj",
                                        "width": 88,
                                        "height": 88
                                    }
                                ]
                            }
                        }
                    }
                }
            }
        });
        assert_eq!(
            find_owner_avatar(&raw).as_deref(),
            Some("https://yt3.ggpht.com/x=s88-c-k-c0x00ffffff-no-rj")
        );
    }
}
