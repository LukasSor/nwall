pub use nwall_ipc::{cache_dir, daemon_log_path, hash_url};

mod archive;
mod bing;
mod cache;
mod coverr;
mod enrich;
mod fetch;
mod github;
mod index;
mod limit;
mod music;
mod nasa;
mod pixabay;
mod sidecar;
mod types;
mod wallhaven;
mod yt_music;

#[cfg(test)]
mod tile_label_tests;

pub use archive::resolve_remote_url;
pub(crate) use archive::{
    archive_agent, archive_pick_file_url, archive_skip_file_name, percent_encode_path,
    pick_archive_file_from_json,
};
pub(crate) use github::github_api_get;
pub use limit::{
    budget_for, err_text_is_rate_limit, err_text_is_timeout, http_err_is_rate_limit,
    http_err_is_timeout, kind_from_url, known_auth, limited_get, limited_get_headers,
    note_source_auth, parse_retry_after, prefetch_delay_for, rate_limit_message,
    should_skip_prefetch, source_detail_concurrency, source_label, source_list_concurrency,
    timeout_message, wait_source_ready, wait_wallhaven_listing_ready, RequestClass, SourceBudget,
};
pub use music::{
    archive_music_starters, download_archive_music, fetch_archive_music,
    fetch_archive_music_duration, resolve_archive_music_download, MusicTrack,
};
pub use wallhaven::WALLHAVEN_RATE_LIMIT_MSG;
pub(crate) use wallhaven::{parse_resolution, urlencoding_lite, WallhavenAvatar, WallhavenItem};
pub use yt_music::{
    download_yt_music, fetch_yt_music, fetch_yt_music_avatar, parse_youtube_video_id,
    parse_yt_music_query, prefetch_yt_music, resolve_yt_music_audio, youtube_music_thumb_url,
    youtube_music_watch_url, yt_dlp_available, YtMusicQuery,
};

pub use cache::*;
pub use enrich::*;
pub use fetch::*;
pub use sidecar::*;
pub use types::*;
