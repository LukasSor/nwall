use anyhow::{anyhow, Result};

use crate::MusicTrack;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum YtMusicQuery {
    Search(String),
    Url(String),
}

pub fn youtube_music_watch_url(id: &str) -> String {
    format!("https://music.youtube.com/watch?v={id}")
}

pub fn youtube_music_thumb_url(id: &str) -> String {
    format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")
}

/// Watch / Shorts / embed / youtu.be / music.youtube.com → 11-char video id.
pub fn parse_youtube_video_id(input: &str) -> Option<String> {
    let u = input.trim();
    if u.is_empty() {
        return None;
    }
    let lower = u.to_ascii_lowercase();
    let yt_host = lower.contains("youtube.com")
        || lower.contains("youtu.be")
        || lower.contains("youtube-nocookie.com");
    if yt_host {
        if let Some(id) = id_from_v_param(u) {
            return valid_video_id(id);
        }
    }
    for (needle, skip) in [
        ("youtu.be/", "youtu.be/".len()),
        ("youtube.com/embed/", "youtube.com/embed/".len()),
        (
            "youtube-nocookie.com/embed/",
            "youtube-nocookie.com/embed/".len(),
        ),
        ("youtube.com/shorts/", "youtube.com/shorts/".len()),
        ("youtube.com/live/", "youtube.com/live/".len()),
        (
            "music.youtube.com/shorts/",
            "music.youtube.com/shorts/".len(),
        ),
    ] {
        if let Some(idx) = lower.find(needle) {
            let start = idx + skip;
            if let Some(raw) = u.get(start..) {
                let id = raw.split(['?', '&', '/', '#']).next().unwrap_or("");
                if let Some(id) = valid_video_id(id) {
                    return Some(id);
                }
            }
        }
    }
    None
}

fn looks_like_youtube_url(input: &str) -> bool {
    let lower = input.trim().to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && (lower.contains("youtube.com/")
            || lower.contains("youtu.be/")
            || lower.contains("youtube-nocookie.com/")
            || lower.contains("music.youtube.com/"))
}

pub fn parse_yt_music_query(input: &str) -> YtMusicQuery {
    let q = input.trim();
    if q.is_empty() {
        return YtMusicQuery::Search(String::new());
    }
    if parse_youtube_video_id(q).is_some() || looks_like_youtube_url(q) {
        YtMusicQuery::Url(q.to_string())
    } else {
        YtMusicQuery::Search(q.to_string())
    }
}

pub(super) fn valid_video_id(id: &str) -> Option<String> {
    let id = id.trim();
    if id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Some(id.to_string())
    } else {
        None
    }
}

fn id_from_v_param(url: &str) -> Option<&str> {
    let rest = url.split_once('?').map(|(_, q)| q)?;
    let rest = rest.split('#').next().unwrap_or(rest);
    rest.split('&').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.eq_ignore_ascii_case("v").then_some(v)
    })
}

pub(super) fn yt_video_id(track: &MusicTrack) -> Result<String> {
    valid_video_id(&track.id)
        .or_else(|| parse_youtube_video_id(&track.page_url))
        .or_else(|| {
            track
                .download_url
                .as_deref()
                .and_then(parse_youtube_video_id)
        })
        .ok_or_else(|| anyhow!("missing YouTube video id"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_watch_and_music_urls() {
        assert_eq!(
            parse_youtube_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            parse_youtube_video_id("https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RD")
                .as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            parse_youtube_video_id("https://youtu.be/dQw4w9WgXcQ?si=abc").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            parse_youtube_video_id("https://www.youtube.com/embed/dQw4w9WgXcQ?rel=0").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            parse_youtube_video_id("https://www.youtube.com/shorts/dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            parse_youtube_video_id("https://youtube-nocookie.com/embed/dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(parse_youtube_video_id("lofi hip hop"), None);
        assert_eq!(
            parse_youtube_video_id("https://archive.org/details/x"),
            None
        );
    }

    #[test]
    fn query_classifies_search_vs_url() {
        assert_eq!(
            parse_yt_music_query("ambient piano"),
            YtMusicQuery::Search("ambient piano".into())
        );
        assert_eq!(
            parse_yt_music_query("  https://music.youtube.com/watch?v=dQw4w9WgXcQ  "),
            YtMusicQuery::Url("https://music.youtube.com/watch?v=dQw4w9WgXcQ".into())
        );
        assert_eq!(
            parse_yt_music_query(""),
            YtMusicQuery::Search(String::new())
        );
    }

    #[test]
    fn watch_and_thumb_urls() {
        assert_eq!(
            youtube_music_watch_url("dQw4w9WgXcQ"),
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(
            youtube_music_thumb_url("dQw4w9WgXcQ"),
            "https://i.ytimg.com/vi/{id}/mqdefault.jpg".replace("{id}", "dQw4w9WgXcQ")
        );
    }
}
