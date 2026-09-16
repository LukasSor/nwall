//! YouTube Music: InnerTube search + player (stream URL / cache). `yt-dlp` only for pasted playlists.

mod avatar;
mod client;
mod download;
mod ids;
mod parse;
mod player;
mod search;

pub use avatar::fetch_yt_music_avatar;
pub use download::{download_yt_music, prefetch_yt_music};
pub use ids::{
    parse_youtube_video_id, parse_yt_music_query, youtube_music_thumb_url, youtube_music_watch_url,
    YtMusicQuery,
};
pub use player::resolve_yt_music_audio;
pub use search::{fetch_yt_music, yt_dlp_available};
