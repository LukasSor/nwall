use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use serde_json::Value;

use super::client::{
    existing_yt_cache, url_cache_get, url_cache_put, wait_yt_slot, yt_err_is_block, yt_http,
    YT_ANDROID_MUSIC_UA, YT_ANDROID_UA, YT_IOS_UA, YT_PLAYER_TIMEOUT, YT_TV_UA,
};
use super::ids::yt_video_id;
use super::parse::{pick_stream_url, playability_err};
use crate::MusicTrack;

struct YtPlayerClient {
    name: &'static str,
    version: &'static str,
    client_id: &'static str,
    ua: &'static str,
    origin: &'static str,
    needs_api_key: bool,
    android_sdk: Option<u32>,
    ios: bool,
    embed: bool,
}

const YT_PLAYER_CLIENTS: &[YtPlayerClient] = &[
    YtPlayerClient {
        name: "ANDROID_MUSIC",
        version: "7.27.52",
        client_id: "21",
        ua: YT_ANDROID_MUSIC_UA,
        origin: "https://music.youtube.com",
        needs_api_key: true,
        android_sdk: Some(30),
        ios: false,
        embed: false,
    },
    YtPlayerClient {
        name: "ANDROID",
        version: "21.26.364",
        client_id: "3",
        ua: YT_ANDROID_UA,
        origin: "https://www.youtube.com",
        needs_api_key: false,
        android_sdk: Some(30),
        ios: false,
        embed: false,
    },
    YtPlayerClient {
        name: "IOS",
        version: "21.26.4",
        client_id: "5",
        ua: YT_IOS_UA,
        origin: "https://www.youtube.com",
        needs_api_key: false,
        android_sdk: None,
        ios: true,
        embed: false,
    },
    YtPlayerClient {
        name: "TVHTML5_SIMPLY_EMBEDDED_PLAYER",
        version: "2.0",
        client_id: "85",
        ua: YT_TV_UA,
        origin: "https://www.youtube.com",
        needs_api_key: false,
        android_sdk: None,
        ios: false,
        embed: true,
    },
];

/// Direct stream URL (or cached file path) for preview / extract via InnerTube player.
pub fn resolve_yt_music_audio(track: &MusicTrack) -> Result<String> {
    let id = yt_video_id(track)?;
    if let Some(hit) = existing_yt_cache(&id) {
        return Ok(hit.to_string_lossy().into_owned());
    }
    if let Some(url) = url_cache_get(&id) {
        return Ok(url);
    }
    let url = innertube_audio_url(&id)?;
    url_cache_put(id, url.clone());
    Ok(url)
}

pub(super) fn innertube_audio_url(video_id: &str) -> Result<String> {
    let mut blocked = false;
    let mut last: Option<anyhow::Error> = None;
    for client in YT_PLAYER_CLIENTS {
        wait_yt_slot();
        match player_audio_url(video_id, client) {
            Ok(url) => return Ok(url),
            Err(e) => {
                if yt_err_is_block(&e.to_string()) {
                    blocked = true;
                }
                last = Some(e);
            }
        }
    }
    if blocked {
        return Err(anyhow!("YouTube blocked this stream"));
    }
    Err(last.unwrap_or_else(|| anyhow!("YouTube blocked this stream")))
}

fn player_audio_url(video_id: &str, client: &YtPlayerClient) -> Result<String> {
    let mut endpoint = format!("{}/youtubei/v1/player?prettyPrint=false", client.origin);
    if client.needs_api_key {
        endpoint.push_str("&key=");
        endpoint.push_str(&innertube_api_key()?);
    }
    let resp = yt_http()
        .post(&endpoint)
        .set("Content-Type", "application/json")
        .set("User-Agent", client.ua)
        .set("Origin", client.origin)
        .set("Referer", &format!("{}/", client.origin))
        .set("X-Goog-Api-Format-Version", "2")
        .set("X-YouTube-Client-Name", client.client_id)
        .set("X-YouTube-Client-Version", client.version)
        .timeout(YT_PLAYER_TIMEOUT)
        .send_json(player_body(video_id, client))
        .map_err(|e| player_http_err(e))?;
    let raw: Value = resp
        .into_json()
        .map_err(|e| anyhow!("YouTube player json: {e}"))?;
    if let Some(err) = playability_err(&raw) {
        return Err(err);
    }
    let Some(sd) = raw.get("streamingData") else {
        return Err(anyhow!("YouTube blocked this stream"));
    };
    pick_stream_url(sd).ok_or_else(|| anyhow!("YouTube blocked this stream"))
}

fn player_body(video_id: &str, client: &YtPlayerClient) -> Value {
    let mut client_obj = serde_json::json!({
        "clientName": client.name,
        "clientVersion": client.version,
        "hl": "en",
        "gl": "US",
        "userAgent": client.ua,
    });
    if let Some(sdk) = client.android_sdk {
        client_obj["androidSdkVersion"] = sdk.into();
        client_obj["osName"] = "Android".into();
        client_obj["osVersion"] = Value::from("11");
    }
    if client.ios {
        client_obj["deviceMake"] = "Apple".into();
        client_obj["deviceModel"] = "iPhone16,2".into();
        client_obj["osName"] = "iPhone".into();
        client_obj["osVersion"] = "18.3.2.22D82".into();
    }
    if client.embed {
        client_obj["clientScreen"] = "EMBED".into();
    }
    let mut context = serde_json::json!({ "client": client_obj });
    if client.embed {
        context["thirdParty"] = serde_json::json!({
            "embedUrl": "https://www.youtube.com/"
        });
    }
    serde_json::json!({
        "context": context,
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "playbackContext": {
            "contentPlaybackContext": {
                "html5Preference": "HTML5_PREF_WANTS"
            }
        }
    })
}

fn innertube_api_key() -> Result<String> {
    static CACHED: OnceLock<String> = OnceLock::new();
    if let Some(key) = CACHED.get() {
        return Ok(key.clone());
    }
    let key = fetch_innertube_api_key()?;
    Ok(CACHED.get_or_init(|| key).clone())
}

fn fetch_innertube_api_key() -> Result<String> {
    const PAGES: &[&str] = &["https://music.youtube.com", "https://www.youtube.com"];
    let mut reached = false;
    for page in PAGES {
        wait_yt_slot();
        match yt_http().get(*page).timeout(YT_PLAYER_TIMEOUT).call() {
            Ok(resp) => {
                reached = true;
                let Ok(body) = resp.into_string() else {
                    continue;
                };
                if let Some(key) = parse_innertube_api_key(&body) {
                    return Ok(key);
                }
            }
            Err(ureq::Error::Status(_, _)) => reached = true,
            Err(ureq::Error::Transport(_)) => {}
        }
    }
    if reached {
        Err(anyhow!("YouTube blocked this stream"))
    } else {
        Err(anyhow!("could not reach YouTube"))
    }
}

fn parse_innertube_api_key(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(i) = rest.find("INNERTUBE_API_KEY") {
        let after = &rest[i + "INNERTUBE_API_KEY".len()..];
        if let Some(key) = key_after_innertube_name(after) {
            return Some(key);
        }
        rest = &after;
    }
    None
}

fn key_after_innertube_name(after: &str) -> Option<String> {
    let colon = after.find(':')?;
    if colon > 6 {
        return None;
    }
    let after = after[colon + 1..].trim_start_matches(['\\', ' ', '"', '\n', '\r', '\t']);
    let end = after.find(['"', '\\', ' ', '\n', '\r', '\t', '<'])?;
    let key = &after[..end];
    if (8..=128).contains(&key.len())
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some(key.to_string())
    } else {
        None
    }
}

fn player_http_err(e: ureq::Error) -> anyhow::Error {
    let s = e.to_string();
    if yt_err_is_block(&s) || s.contains("401") || s.contains("403") {
        anyhow!("YouTube blocked this stream")
    } else {
        anyhow!("YouTube Music player: {e}")
    }
}

#[cfg(test)]
mod tests {
    use super::parse_innertube_api_key;

    #[test]
    fn parses_innertube_api_key_from_ytcfg() {
        let html = r#"<script>ytcfg.set({"INNERTUBE_API_KEY":"test-key-not-real"});</script>"#;
        assert_eq!(
            parse_innertube_api_key(html).as_deref(),
            Some("test-key-not-real")
        );
        assert!(parse_innertube_api_key("<html>no key</html>").is_none());
    }
}
