use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::ids::youtube_music_watch_url;
use crate::{cached_path, MusicTrack};

pub(super) const YT_MUSIC_ROWS: usize = 8;
pub(super) const YT_SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const YT_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(90);
pub(super) const YT_MIN_INTERVAL: Duration = Duration::from_millis(400);
pub(super) const YT_MAX_FILE_BYTES: u64 = 40_000_000;
pub(super) const YT_SEARCH_TTL: Duration = Duration::from_secs(30 * 60);
pub(super) const YT_URL_TTL: Duration = Duration::from_secs(3 * 60 * 60);
pub(super) const YT_SEARCH_CACHE_MAX: usize = 32;

pub(super) const YT_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
pub(super) const YT_PLAYER_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) const YT_ANDROID_UA: &str =
    "com.google.android.youtube/21.26.364 (Linux; U; Android 11) gzip";
pub(super) const YT_ANDROID_MUSIC_UA: &str =
    "com.google.android.apps.youtube.music/7.27.52 (Linux; U; Android 11) gzip";
pub(super) const YT_IOS_UA: &str =
    "com.google.ios.youtube/21.26.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)";
pub(super) const YT_TV_UA: &str = "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version";

pub(super) const YT_AUDIO_EXTS: &[&str] = &[
    "m4a", "webm", "opus", "ogg", "mp3", "aac", "m4b", "weba", "mp4",
];

pub(super) fn yt_http() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(YT_UA)
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(25))
            .timeout(Duration::from_secs(30))
            .build()
    })
}

pub(super) struct YtMem {
    pub(super) search: HashMap<String, (Instant, Vec<MusicTrack>)>,
    pub(super) search_order: VecDeque<String>,
    pub(super) urls: HashMap<String, (Instant, String)>,
    pub(super) avatars: HashMap<String, Option<String>>,
}

pub(super) fn yt_mem() -> &'static Mutex<YtMem> {
    static MEM: OnceLock<Mutex<YtMem>> = OnceLock::new();
    MEM.get_or_init(|| {
        Mutex::new(YtMem {
            search: HashMap::new(),
            search_order: VecDeque::new(),
            urls: HashMap::new(),
            avatars: HashMap::new(),
        })
    })
}

pub(super) fn search_cache_key(q: &str) -> String {
    q.split_whitespace()
        .map(|s| s.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn search_cache_get(key: &str) -> Option<Vec<MusicTrack>> {
    let mut g = yt_mem().lock().ok()?;
    let (at, tracks) = g.search.get(key)?;
    if at.elapsed() > YT_SEARCH_TTL {
        g.search.remove(key);
        g.search_order.retain(|k| k != key);
        return None;
    }
    Some(tracks.clone())
}

pub(super) fn search_cache_put(key: String, tracks: Vec<MusicTrack>) {
    let Ok(mut g) = yt_mem().lock() else {
        return;
    };
    if g.search
        .insert(key.clone(), (Instant::now(), tracks))
        .is_none()
    {
        g.search_order.push_back(key);
        while g.search_order.len() > YT_SEARCH_CACHE_MAX {
            if let Some(old) = g.search_order.pop_front() {
                g.search.remove(&old);
            }
        }
    }
}

pub(super) fn url_cache_get(id: &str) -> Option<String> {
    let mut g = yt_mem().lock().ok()?;
    let (at, url) = g.urls.get(id)?;
    if at.elapsed() > YT_URL_TTL || !googlevideo_fresh(url) {
        g.urls.remove(id);
        return None;
    }
    Some(url.clone())
}

pub(super) fn url_cache_put(id: String, url: String) {
    if let Ok(mut g) = yt_mem().lock() {
        g.urls.insert(id, (Instant::now(), url));
    }
}

pub(super) fn avatar_cache_get(id: &str) -> Option<Option<String>> {
    let g = yt_mem().lock().ok()?;
    g.avatars.get(id).cloned()
}

pub(super) fn avatar_cache_put(id: String, url: Option<String>) {
    if let Ok(mut g) = yt_mem().lock() {
        g.avatars.insert(id, url);
    }
}

fn googlevideo_fresh(url: &str) -> bool {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or(url);
    let Some(exp) = query.split('&').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.eq_ignore_ascii_case("expire").then_some(v)
    }) else {
        return true;
    };
    let Ok(exp) = exp.parse::<u64>() else {
        return true;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    exp > now.saturating_add(120)
}

pub(super) fn yt_id_lock(id: &str) -> std::sync::Arc<Mutex<()>> {
    static MAP: OnceLock<Mutex<HashMap<String, std::sync::Arc<Mutex<()>>>>> = OnceLock::new();
    let map = MAP.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().unwrap_or_else(|e| e.into_inner());
    g.entry(id.to_string())
        .or_insert_with(|| std::sync::Arc::new(Mutex::new(())))
        .clone()
}

pub(super) fn wait_yt_slot() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    loop {
        let wait = {
            let mut g = LAST.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(last) = *g {
                let elapsed = last.elapsed();
                if elapsed < YT_MIN_INTERVAL {
                    YT_MIN_INTERVAL - elapsed
                } else {
                    *g = Some(Instant::now());
                    return;
                }
            } else {
                *g = Some(Instant::now());
                return;
            }
        };
        thread::sleep(wait.min(Duration::from_millis(250)));
    }
}

pub(super) fn existing_yt_cache(id: &str) -> Option<PathBuf> {
    let url = youtube_music_watch_url(id);
    let dest = cached_path(&url, "ytmusic");
    if dest.is_file() && dest.metadata().ok()?.len() > 1024 {
        return Some(dest);
    }
    for ext in YT_AUDIO_EXTS {
        let p = dest.with_extension(ext);
        if p.is_file() && p.metadata().ok()?.len() > 1024 {
            return Some(p);
        }
    }
    None
}

pub(super) fn yt_err_is_block(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("sign in")
        || l.contains("not a bot")
        || l.contains("login_required")
        || l.contains("blocked this stream")
        || l.contains("no longer supported")
        || l.contains("confirm you’re not")
        || l.contains("confirm you're not")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expire_param_freshness() {
        assert!(googlevideo_fresh("https://x.example/a?foo=1"));
        assert!(!googlevideo_fresh("https://x.example/a?expire=1"));
        let future = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        assert!(googlevideo_fresh(&format!(
            "https://x.example/a?expire={future}&itag=251"
        )));
    }

    #[test]
    fn search_key_collapses_ws() {
        assert_eq!(search_cache_key("  LoFi   Hip  Hop "), "lofi hip hop");
    }
}
