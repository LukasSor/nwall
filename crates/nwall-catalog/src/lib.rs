
pub use nwall_ipc::{cache_dir, daemon_log_path, hash_url};

mod types;
mod enrich;
mod sidecar;
mod cache;
mod fetch;
mod wallhaven;
mod bing;
mod pixabay;
mod coverr;
mod nasa;
mod archive;
mod music;
mod github;
mod index;

#[cfg(test)]
mod tile_label_tests;

pub(crate) use archive::{
    archive_agent, archive_pick_file_url, archive_skip_file_name, percent_encode_path,
    pick_archive_file_from_json,
};
pub use archive::resolve_remote_url;
pub use music::{
    archive_music_starters, download_archive_music, fetch_archive_music,
    fetch_archive_music_duration, resolve_archive_music_download, MusicTrack,
};
pub(crate) use github::github_api_get;
pub(crate) use wallhaven::{
    parse_resolution, urlencoding_lite, wallhaven_remote_item, WallhavenAvatar, WallhavenItem,
    WallhavenTag, WallhavenUploader,
};
pub(crate) use bing::{
    bing_format_startdate, bing_id_from_urlbase, bing_item, bing_split_copyright, BingImage,
};
pub(crate) use nasa::{
    nasa_apod_page_url, nasa_file_type_from_url, nasa_item, nasa_youtube_watch_url, NasaApod,
};
pub(crate) use github::{
    github_blob_download_url, github_path_is_direct_child, github_size_looks_like_lfs_pointer,
};

pub use types::*;
pub use enrich::*;
pub use sidecar::*;
pub use cache::*;
pub use fetch::*;
