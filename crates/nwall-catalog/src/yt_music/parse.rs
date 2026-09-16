use serde::Deserialize;
use serde_json::Value;

use super::avatar::{remix_channel_avatar, yt_json_avatar};
use super::client::{yt_err_is_block, YT_MUSIC_ROWS};
use super::ids::{
    parse_youtube_video_id, valid_video_id, youtube_music_thumb_url, youtube_music_watch_url,
};
use crate::{json_stringish, parse_archive_runtime, MusicTrack};

const YT_AUDIO_ITAGS: &[i64] = &[140, 251, 250, 249, 141, 139, 599, 600];
const YT_MUXED_ITAGS: &[i64] = &[18, 22];

pub(super) fn collect_remix_tracks(
    v: &Value,
    out: &mut Vec<MusicTrack>,
    seen: &mut std::collections::HashSet<String>,
) {
    if out.len() >= YT_MUSIC_ROWS {
        return;
    }
    match v {
        Value::Object(map) => {
            if let Some(item) = map.get("musicResponsiveListItemRenderer") {
                if let Some(track) = track_from_remix_item(item) {
                    if seen.insert(track.id.clone()) {
                        out.push(track);
                    }
                }
                return;
            }
            for val in map.values() {
                collect_remix_tracks(val, out, seen);
                if out.len() >= YT_MUSIC_ROWS {
                    return;
                }
            }
        }
        Value::Array(arr) => {
            for val in arr {
                collect_remix_tracks(val, out, seen);
                if out.len() >= YT_MUSIC_ROWS {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn track_from_remix_item(item: &Value) -> Option<MusicTrack> {
    let id = item
        .pointer("/playlistItemData/videoId")
        .and_then(|v| v.as_str())
        .or_else(|| {
            item.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId")
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            item.pointer("/doubleTapCommand/watchEndpoint/videoId")
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            item.pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/navigationEndpoint/watchEndpoint/videoId")
                .and_then(|v| v.as_str())
        })
        .and_then(valid_video_id)?;
    let title = item
        .pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/text")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| id.clone());
    let mut creator = None;
    let mut duration = None;
    if let Some(runs) = item
        .pointer("/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text/runs")
        .and_then(|v| v.as_array())
    {
        for run in runs {
            let text = run
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if text.is_empty() || text == "•" || text == "·" {
                continue;
            }
            if duration.is_none() {
                if let Some(d) = parse_archive_runtime(text) {
                    duration = Some(d);
                    continue;
                }
            }
            if creator.is_none() {
                creator = Some(text.to_string());
            }
        }
    }
    Some(MusicTrack {
        page_url: youtube_music_watch_url(&id),
        title,
        creator,
        duration,
        download_url: Some(youtube_music_watch_url(&id)),
        size: None,
        license_url: None,
        thumb_url: Some(youtube_music_thumb_url(&id)),
        avatar: remix_channel_avatar(item),
        id,
    })
}

#[derive(Deserialize)]
struct YtFlat {
    #[serde(default)]
    id: Option<String>,
    title: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    artist: Option<Value>,
    #[serde(default)]
    duration: Option<Value>,
    #[serde(default)]
    webpage_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    is_live: Option<bool>,
    #[serde(default)]
    live_status: Option<String>,
}

pub(super) fn track_from_yt_json(raw: &Value) -> Option<MusicTrack> {
    let parsed: YtFlat = serde_json::from_value(raw.clone()).ok()?;
    if parsed.is_live.unwrap_or(false) {
        return None;
    }
    if parsed
        .live_status
        .as_deref()
        .is_some_and(|s| s == "is_live" || s == "is_upcoming")
    {
        return None;
    }
    let id = parsed
        .id
        .as_deref()
        .and_then(valid_video_id)
        .or_else(|| {
            parsed
                .webpage_url
                .as_deref()
                .and_then(parse_youtube_video_id)
        })
        .or_else(|| parsed.url.as_deref().and_then(parse_youtube_video_id))?;
    let title = parsed
        .title
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| id.clone());
    let creator = json_stringish(parsed.artist.as_ref())
        .or_else(|| parsed.uploader.clone())
        .or_else(|| parsed.channel.clone())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let duration = json_stringish(parsed.duration.as_ref())
        .and_then(|s| s.parse().ok())
        .or_else(|| parsed.duration.as_ref().and_then(|v| v.as_f64()))
        .filter(|d| d.is_finite() && *d > 0.0);
    let thumb = parsed
        .thumbnail
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| youtube_music_thumb_url(&id));
    Some(MusicTrack {
        page_url: youtube_music_watch_url(&id),
        title,
        creator,
        duration,
        download_url: Some(youtube_music_watch_url(&id)),
        size: None,
        license_url: None,
        thumb_url: Some(thumb),
        avatar: yt_json_avatar(raw),
        id,
    })
}

pub(super) fn playability_err(raw: &Value) -> Option<anyhow::Error> {
    let status = raw
        .pointer("/playabilityStatus/status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if status.is_empty() || status == "OK" {
        return None;
    }
    let reason = raw
        .pointer("/playabilityStatus/reason")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Some(playability_message(status, reason))
}

fn playability_message(status: &str, reason: &str) -> anyhow::Error {
    let blob = format!("{status} {reason}");
    if yt_err_is_block(&blob) {
        anyhow::anyhow!("YouTube blocked this stream")
    } else if !reason.trim().is_empty() {
        anyhow::anyhow!("{}", reason.trim())
    } else {
        anyhow::anyhow!("YouTube blocked this stream")
    }
}

pub(super) fn pick_stream_url(sd: &Value) -> Option<String> {
    let mut audio: Vec<(i64, String)> = Vec::new();
    let mut muxed: Vec<(i64, String)> = Vec::new();
    for key in ["adaptiveFormats", "formats"] {
        let Some(arr) = sd.get(key).and_then(|v| v.as_array()) else {
            continue;
        };
        for f in arr {
            let Some(url) = f
                .get("url")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
            else {
                continue;
            };
            let itag = f.get("itag").and_then(|v| v.as_i64()).unwrap_or(0);
            let mime = f
                .get("mimeType")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if YT_AUDIO_ITAGS.contains(&itag) || mime.starts_with("audio/") {
                audio.push((itag, url.to_string()));
            } else if YT_MUXED_ITAGS.contains(&itag) || mime.contains("mp4a") {
                muxed.push((itag, url.to_string()));
            }
        }
    }
    for want in YT_AUDIO_ITAGS {
        if let Some((_, url)) = audio.iter().find(|(itag, _)| itag == want) {
            return Some(url.clone());
        }
    }
    if let Some((_, url)) = audio.into_iter().next() {
        return Some(url);
    }
    for want in YT_MUXED_ITAGS {
        if let Some((_, url)) = muxed.iter().find(|(itag, _)| itag == want) {
            return Some(url.clone());
        }
    }
    muxed.into_iter().next().map(|(_, url)| url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_from_search_json() {
        let raw = serde_json::json!({
            "id": "dQw4w9WgXcQ",
            "title": "Never Gonna Give You Up",
            "uploader": "Rick Astley",
            "duration": 213.0,
            "webpage_url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "is_live": false
        });
        let t = track_from_yt_json(&raw).expect("track");
        assert_eq!(t.id, "dQw4w9WgXcQ");
        assert_eq!(t.title, "Never Gonna Give You Up");
        assert_eq!(t.creator.as_deref(), Some("Rick Astley"));
        assert_eq!(t.duration, Some(213.0));
        assert_eq!(t.page_url, "https://music.youtube.com/watch?v=dQw4w9WgXcQ");
        assert!(t.thumb_url.as_deref().unwrap().contains("dQw4w9WgXcQ"));
    }

    #[test]
    fn skips_live_and_bad_ids() {
        let live = serde_json::json!({
            "id": "dQw4w9WgXcQ",
            "title": "live",
            "is_live": true
        });
        assert!(track_from_yt_json(&live).is_none());
        let bad = serde_json::json!({ "id": "nope", "title": "x" });
        assert!(track_from_yt_json(&bad).is_none());
    }

    #[test]
    fn remix_item_parses_song_row() {
        let item = serde_json::json!({
            "playlistItemData": { "videoId": "dQw4w9WgXcQ" },
            "flexColumns": [
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": { "runs": [{ "text": "Never Gonna Give You Up" }] }
                    }
                },
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": { "runs": [
                            { "text": "Rick Astley" },
                            { "text": " • " },
                            { "text": "Whenever You Need Somebody" },
                            { "text": " • " },
                            { "text": "3:33" }
                        ] }
                    }
                }
            ]
        });
        let t = track_from_remix_item(&item).expect("track");
        assert_eq!(t.id, "dQw4w9WgXcQ");
        assert_eq!(t.title, "Never Gonna Give You Up");
        assert_eq!(t.creator.as_deref(), Some("Rick Astley"));
        assert_eq!(t.duration, Some(213.0));
        assert_eq!(
            t.thumb_url.as_deref(),
            Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg")
        );
        assert!(t.avatar.is_none());
    }

    #[test]
    fn remix_channel_thumbnail_is_avatar_not_cover() {
        let item = serde_json::json!({
            "playlistItemData": { "videoId": "dQw4w9WgXcQ" },
            "flexColumns": [
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": { "runs": [{ "text": "Never Gonna Give You Up" }] }
                    }
                },
                {
                    "musicResponsiveListItemFlexColumnRenderer": {
                        "text": { "runs": [{ "text": "Rick Astley" }] }
                    }
                }
            ],
            "thumbnail": {
                "musicThumbnailRenderer": {
                    "thumbnail": {
                        "thumbnails": [{
                            "url": "https://yt3.googleusercontent.com/albumhash=w60-h60-l90-rj",
                            "width": 60,
                            "height": 60
                        }]
                    }
                }
            },
            "channelThumbnailSupportedRenderers": {
                "channelThumbnailWithLinkRenderer": {
                    "thumbnail": {
                        "thumbnails": [{
                            "url": "https://yt3.ggpht.com/chanhash=s88-c-k-c0x00ffffff-no-rj",
                            "width": 88,
                            "height": 88
                        }]
                    }
                }
            }
        });
        let t = track_from_remix_item(&item).expect("track");
        assert_eq!(
            t.thumb_url.as_deref(),
            Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg")
        );
        assert_eq!(
            t.avatar.as_deref(),
            Some("https://yt3.ggpht.com/chanhash=s88-c-k-c0x00ffffff-no-rj")
        );
    }

    #[test]
    fn remix_skips_artist_rows() {
        let item = serde_json::json!({
            "navigationEndpoint": { "browseEndpoint": { "browseId": "UC123" } },
            "flexColumns": [{
                "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": [{ "text": "An Artist" }] }
                }
            }]
        });
        assert!(track_from_remix_item(&item).is_none());
    }

    #[test]
    fn pick_prefers_itag_140_then_251() {
        let sd = serde_json::json!({
            "adaptiveFormats": [
                {"itag": 18, "url": "https://x/18", "mimeType": "video/mp4"},
                {"itag": 251, "url": "https://x/251", "mimeType": "audio/webm"},
                {"itag": 140, "url": "https://x/140", "mimeType": "audio/mp4"}
            ]
        });
        assert_eq!(pick_stream_url(&sd).as_deref(), Some("https://x/140"));
    }

    #[test]
    fn pick_muxed_18_when_adaptive_has_no_url() {
        let sd = serde_json::json!({
            "adaptiveFormats": [
                {"itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\""},
                {"itag": 251, "mimeType": "audio/webm; codecs=\"opus\""}
            ],
            "formats": [
                {"itag": 18, "url": "https://x/18", "mimeType": "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\""}
            ]
        });
        assert_eq!(pick_stream_url(&sd).as_deref(), Some("https://x/18"));
    }

    #[test]
    fn pick_skips_signature_cipher() {
        let sd = serde_json::json!({
            "adaptiveFormats": [
                {"itag": 140, "signatureCipher": "s=abc&url=https://x/140", "mimeType": "audio/mp4"}
            ]
        });
        assert!(pick_stream_url(&sd).is_none());
    }

    #[test]
    fn bot_reasons_are_human_block() {
        assert!(yt_err_is_block("LOGIN_REQUIRED Please sign in"));
        assert!(yt_err_is_block(
            "ERROR: [youtube] QpXq0E_ZnP4: Sign in to confirm you're not a bot"
        ));
        assert_eq!(
            playability_message("LOGIN_REQUIRED", "Please sign in").to_string(),
            "YouTube blocked this stream"
        );
    }
}
