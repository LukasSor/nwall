use std::path::Path;

use nwall_ipc::{is_image, is_video, CatalogSource};

use super::bing::{
    bing_format_startdate, bing_id_from_urlbase, bing_item, bing_split_copyright, BingImage,
};
use super::github::{
    github_blob_download_url, github_path_is_direct_child, github_size_looks_like_lfs_pointer,
};
use super::nasa::{
    nasa_apod_page_url, nasa_file_type_from_url, nasa_item, nasa_youtube_watch_url, NasaApod,
};
use super::wallhaven::{wallhaven_remote_item, WallhavenTag, WallhavenUploader};
use super::*;

fn wallhaven_item(name: &str, id: &str, w: u32, h: u32, cat: &str) -> RemoteItem {
    RemoteItem {
        name: name.into(),
        url: "https://w.wallhaven.cc/full/nm/wallhaven-nmmvvk.jpg".into(),
        credit: Some("Wallhaven".into()),
        id: Some(id.into()),
        width: Some(w),
        height: Some(h),
        category: Some(cat.into()),
        ..Default::default()
    }
}

#[test]
fn caption_uses_name_not_resolution() {
    let it = wallhaven_item("Matterhorn", "nmmvvk", 2048, 1280, "general");
    assert_eq!(it.tile_label(), "Matterhorn");
    assert_eq!(it.resolution_meta(), "2048×1280 · General");
    let tip = it.tooltip_label();
    assert!(tip.contains("2048×1280 · General · #nmmvvk"), "{tip}");
    assert!(!it.tile_label().contains('×'));
}

#[test]
fn stale_resolution_caption_falls_back_to_id() {
    let it = wallhaven_item("2048×1280 · general", "nmmvvk", 2048, 1280, "general");
    assert_eq!(it.tile_label(), "#nmmvvk");
    assert_eq!(it.resolution_meta(), "2048×1280 · General");
}

#[test]
fn empty_browse_caption_uses_category() {
    let w = WallhavenItem {
        id: "zztest1".into(),
        path: "https://w.wallhaven.cc/full/zz/wallhaven-zztest1.jpg".into(),
        resolution: "2048x1280".into(),
        category: "general".into(),
        purity: "sfw".into(),
        ..Default::default()
    };
    let it = wallhaven_remote_item(w, "");
    assert_eq!(it.tile_label(), "General");
    assert_eq!(it.resolution_meta(), "2048×1280 · General");
}

#[test]
fn search_query_becomes_caption() {
    let w = WallhavenItem {
        id: "zztest2".into(),
        path: "https://w.wallhaven.cc/full/zz/wallhaven-zztest2.jpg".into(),
        resolution: "1920x1080".into(),
        category: "general".into(),
        purity: "sfw".into(),
        ..Default::default()
    };
    let it = wallhaven_remote_item(w, "Matterhorn");
    assert_eq!(it.tile_label(), "Matterhorn");
    assert!(it
        .tooltip_label()
        .contains("1920×1080 · General · #zztest2"));
}

#[test]
fn tags_win_over_query() {
    let w = WallhavenItem {
        id: "abc123".into(),
        path: "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg".into(),
        resolution: "1920x1080".into(),
        category: "general".into(),
        purity: "sfw".into(),
        tags: vec![
            WallhavenTag {
                name: "alps".into(),
            },
            WallhavenTag {
                name: "snow".into(),
            },
            WallhavenTag {
                name: "matterhorn".into(),
            },
        ],
        ..Default::default()
    };
    let it = wallhaven_remote_item(w, "Matterhorn");
    assert_eq!(it.tile_label(), "alps · snow · matterhorn");
    assert_eq!(it.tags.len(), 3);
}

#[test]
fn stats_format_skips_unknown() {
    let s = MediaStats {
        width: Some(1920),
        height: Some(1080),
        file_size: Some(2_400_000),
        tags: vec!["alps".into(), "snow".into()],
        category: Some("general".into()),
        purity: Some("sfw".into()),
        uploader: Some("alice".into()),
        ..Default::default()
    };
    let t = s.format();
    assert!(t.contains("Resolution: 1920×1080"), "{t}");
    assert!(
        t.contains("Size: 2.3 MB") || t.contains("Size: 2.4 MB"),
        "{t}"
    );
    assert!(t.contains("Tags: alps · snow"), "{t}");
    assert!(t.contains("Category: General"), "{t}");
    assert!(t.contains("Purity: SFW"), "{t}");
    assert!(t.contains("Uploader: alice"), "{t}");
    assert!(!t.contains("Duration:"), "{t}");
    assert!(!t.contains("Length:"), "{t}");
    assert!(!t.contains("Views:"), "{t}");
    assert!(!t.contains("Favorites:"), "{t}");
}

#[test]
fn wallhaven_search_hit_maps_views_favorites_link_colors() {
    let w = WallhavenItem {
        id: "abc123".into(),
        path: "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg".into(),
        url: "https://wallhaven.cc/w/abc123".into(),
        short_url: "http://whvn.cc/abc123".into(),
        views: Some(12345),
        favorites: Some(67),
        colors: vec!["#000000".into(), "#ABBCda".into(), "nope".into()],
        resolution: "1920x1080".into(),
        category: "general".into(),
        purity: "sfw".into(),
        ..Default::default()
    };
    let it = wallhaven_remote_item(w, "");
    assert_eq!(it.views, Some(12345));
    assert_eq!(it.favorites, Some(67));
    assert_eq!(it.page_url.as_deref(), Some("https://whvn.cc/abc123"));
    assert_eq!(
        it.colors,
        vec!["#000000".to_string(), "#abbcda".to_string()]
    );
    let t = it.stats().format_body();
    assert!(t.contains("Views: 12345"), "{t}");
    assert!(t.contains("Favorites: 67"), "{t}");
    assert!(!t.contains("Link:"), "{t}");
}

#[test]
fn wallhaven_page_url_falls_back_to_id() {
    let w = WallhavenItem {
        id: "zztest1".into(),
        ..Default::default()
    };
    assert_eq!(
        wallhaven_page_url(&w).as_deref(),
        Some("https://wallhaven.cc/w/zztest1")
    );
}

#[test]
fn source_label_maps_known_hosts() {
    let mut it = RemoteItem {
        url: "https://w.wallhaven.cc/full/ab/wallhaven-abc.jpg".into(),
        credit: Some("Wallhaven".into()),
        ..Default::default()
    };
    assert_eq!(item_source_label(&it), "Wallhaven");
    it.url = "https://www.bing.com/th?id=OHR.Foo".into();
    it.credit = Some("Jean-Marie Tjibaou Cultural Centre (© Fabien Astre/Alamy)".into());
    assert_eq!(item_source_label(&it), "Bing Daily");
    it.url = "https://archive.org/download/foo".into();
    it.credit = Some("Internet Archive".into());
    assert_eq!(item_source_label(&it), "Archive.org");
}

#[test]
fn wallhaven_avatar_prefers_32px() {
    let av = WallhavenAvatar {
        px32: Some("https://example/32.jpg".into()),
        px128: Some("https://example/128.jpg".into()),
        px200: Some("https://example/200.jpg".into()),
    };
    assert_eq!(
        wallhaven_avatar_url(&av).as_deref(),
        Some("https://example/32.jpg")
    );
    let w = WallhavenItem {
        id: "abc123".into(),
        uploader: Some(WallhavenUploader {
            username: "alice".into(),
            avatar: Some(av),
        }),
        ..Default::default()
    };
    let d = wallhaven_details_from_item(w);
    assert_eq!(d.uploader.as_deref(), Some("alice"));
    assert_eq!(d.avatar.as_deref(), Some("https://example/32.jpg"));
}

#[test]
fn human_bytes_and_duration() {
    assert_eq!(human_bytes(500), "500 B");
    assert_eq!(human_bytes(1024), "1 KB");
    assert_eq!(format_duration(72.4), "1:12");
    assert_eq!(format_duration(3661.0), "1:01:01");
}

#[test]
fn tag_file_stem_sanitizes() {
    let tags = vec![
        "matterhorn".into(),
        "alps".into(),
        "snow".into(),
        "extra".into(),
    ];
    assert_eq!(
        tag_file_stem(&tags, 3).as_deref(),
        Some("matterhorn-alps-snow")
    );
    assert_eq!(sanitize_file_stem("foo/bar · baz.jpg"), "foo-bar-baz-jpg");
    assert!(looks_like_opaque_stem("wx8gqr"));
    assert!(looks_like_opaque_stem("2048x1280-general"));
    assert!(!looks_like_opaque_stem("matterhorn-alps-snow"));
}

#[test]
fn library_stem_prefers_tags_over_id() {
    let mut it = wallhaven_item("Matterhorn", "nmmvvk", 2048, 1280, "general");
    it.tags = vec!["matterhorn".into(), "alps".into(), "snow".into()];
    assert_eq!(it.library_file_stem(), "matterhorn-alps-snow");
    let bare = wallhaven_item("#nmmvvk", "nmmvvk", 2048, 1280, "general");
    assert_eq!(bare.library_file_stem(), "nmmvvk");
}

#[test]
fn github_keeps_descriptive_name() {
    let it = RemoteItem {
        name: "Sunset_Over_Alps.jpg".into(),
        url: "https://raw.githubusercontent.com/u/r/Sunset_Over_Alps.jpg".into(),
        ..Default::default()
    };
    assert_eq!(it.library_file_stem(), "Sunset_Over_Alps");
    assert_eq!(
        display_caption(None, "Sunset_Over_Alps.jpg"),
        "Sunset_Over_Alps.jpg"
    );
}

#[test]
fn github_repo_topics_fill_tags_keep_tile_name() {
    let mut it = RemoteItem {
        name: "Sunset_Over_Alps.jpg".into(),
        url: "https://raw.githubusercontent.com/u/r/Sunset_Over_Alps.jpg".into(),
        repo: Some("owner/walls".into()),
        id: Some("Sunset_Over_Alps.jpg".into()),
        ..Default::default()
    };
    let d = GitHubDetails {
        tags: vec!["wallpaper".into(), "nature".into(), "linux".into()],
        license: Some("MIT".into()),
        author: Some("octocat".into()),
        repo: Some("owner/walls".into()),
        ..Default::default()
    };
    it.apply_github_details(&d);
    assert_eq!(it.tags, vec!["wallpaper", "nature", "linux"]);
    assert_eq!(it.tile_label(), "Sunset_Over_Alps.jpg");
    assert_eq!(it.library_file_stem(), "Sunset_Over_Alps");
    let stats = it.stats();
    assert_eq!(stats.tags, vec!["wallpaper", "nature", "linux"]);
    assert_eq!(
        display_caption(Some(&stats), "Sunset_Over_Alps.jpg"),
        "Sunset_Over_Alps.jpg"
    );
    let topics = parse_github_topics(&serde_json::json!({
        "topics": ["Wallpaper", "  ", "desktop"]
    }));
    assert_eq!(topics, vec!["Wallpaper", "desktop"]);
}

#[test]
fn github_commit_user_gets_png_avatar() {
    let raw = serde_json::json!([{
        "author": {
            "login": "octocat",
            "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4"
        },
        "commit": { "author": { "name": "The Octocat", "date": "2024-01-02T00:00:00Z" } }
    }]);
    let d = github_details_from_commits(&raw, "octo/repo");
    assert_eq!(d.author.as_deref(), Some("octocat"));
    assert_eq!(
        d.avatar.as_deref(),
        Some("https://github.com/octocat.png?size=64")
    );
    assert_eq!(d.date.as_deref(), Some("2024-01-02"));
    assert_eq!(d.repo.as_deref(), Some("octo/repo"));
}

#[test]
fn github_anonymous_commit_has_name_no_avatar() {
    let raw = serde_json::json!([{
        "author": null,
        "commit": { "author": { "name": "Anon Dev", "date": "2024-06-15T12:00:00Z" } }
    }]);
    let d = github_details_from_commits(&raw, "o/r");
    assert_eq!(d.author.as_deref(), Some("Anon Dev"));
    assert!(d.avatar.is_none(), "{:?}", d.avatar);
}

#[test]
fn avatar_cache_path_uses_image_ext() {
    let p = cached_path("https://avatars.githubusercontent.com/u/1?v=4", "avatar");
    assert!(
        p.extension().and_then(|e| e.to_str()) == Some("png"),
        "{}",
        p.display()
    );
}

#[test]
fn display_caption_uses_tags_not_id_filename() {
    let meta = MediaStats {
        tags: vec!["matterhorn".into(), "alps".into(), "snow".into()],
        title: Some("Matterhorn".into()),
        source_id: Some("wx8gqr".into()),
        ..Default::default()
    };
    assert_eq!(
        display_caption(Some(&meta), "wx8gqr.jpg"),
        "matterhorn · alps · snow"
    );
    let titled = MediaStats {
        title: Some("Bing Daily Alps".into()),
        ..Default::default()
    };
    assert_eq!(
        display_caption(Some(&titled), "OHR.Alps_UHD.jpg"),
        "Bing Daily Alps"
    );
}

#[test]
fn pick_library_dest_suffixes_on_collision() {
    let dir = std::env::temp_dir().join(format!(
        "nwall-libdest-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let primary = dir.join("alps-snow.jpg");
    std::fs::write(&primary, b"one").unwrap();
    let other = RemoteItem {
        name: "other".into(),
        id: Some("aaaaaa".into()),
        tags: vec!["alps".into(), "snow".into()],
        ..Default::default()
    };
    persist_remote_meta(&other, &primary);
    let dest = pick_library_dest(&dir, "alps-snow", "jpg", Some("wx8gqr"));
    assert_eq!(dest, dir.join("alps-snow-wx8gqr.jpg"));
    let same = pick_library_dest(&dir, "alps-snow", "jpg", Some("aaaaaa"));
    assert_eq!(same, primary);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bing_query_url_is_jpg_not_bin() {
    assert_eq!(
        ext_from_url("https://www.bing.com/th?id=OHR.KochiaChina_EN-US5037126636_UHD.jpg"),
        "jpg"
    );
    assert_eq!(
        ext_from_url("https://www.bing.com/th?id=OHR.KochiaChina_EN-US5037126636"),
        "jpg"
    );
    assert_eq!(ext_from_url("https://example.com/a.jpg?token=1"), "jpg");
    assert_eq!(ext_from_url("https://example.com/download/abc"), "bin");
    assert_eq!(
        ext_from_content_type("image/jpeg; charset=binary"),
        Some("jpg")
    );
    assert_eq!(ext_from_magic(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
    assert_eq!(
        ext_from_magic(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        Some("png")
    );
    assert_eq!(
        stem_from_download_hint("OHR.KochiaChina_EN-US5037126636"),
        "OHR-KochiaChina_EN-US5037126636"
    );
    let dest = pick_library_dest(
        Path::new("/tmp"),
        &stem_from_download_hint("OHR.KochiaChina_EN-US5037126636"),
        ext_from_url("https://www.bing.com/th?id=OHR.KochiaChina_EN-US5037126636"),
        Some("OHR.KochiaChina_EN-US5037126636"),
    );
    assert_eq!(
        dest.file_name().and_then(|n| n.to_str()),
        Some("OHR-KochiaChina_EN-US5037126636.jpg")
    );
    assert!(!is_image(Path::new(
        "OHR-OHR-KochiaChina_EN-US5037126636.bin"
    )));
    assert!(is_image(&dest), "{}", dest.display());
}

#[test]
fn coverr_storage_video_url_is_mp4_not_bin() {
    assert_eq!(
        ext_from_url("https://storage.coverr.co/videos/QatsCWWAorI71sZ33DkHZREGWruZCHsg?token=abc"),
        "mp4"
    );
    assert_eq!(
        ext_from_url(
            "https://storage.coverr.co/videos/QatsCWWAorI71sZ33DkHZREGWruZCHsg/download?token=abc&filename=Cutting%20Wood"
        ),
        "mp4"
    );
    assert_eq!(
        ext_from_url(
            "https://storage.coverr.co/videos/QatsCWWAorI71sZ33DkHZREGWruZCHsg/preview?token=abc"
        ),
        "mp4"
    );
    assert_eq!(
        ext_from_url("https://storage.coverr.co/t/QatsCWWAorI71sZ33DkHZREGWruZCHsg"),
        "bin"
    );
    assert_eq!(
        ext_from_url("https://storage.coverr.co/p/QatsCWWAorI71sZ33DkHZREGWruZCHsg"),
        "bin"
    );
    assert_eq!(
        ext_from_url("https://api.coverr.co/videos?urls=true"),
        "bin"
    );
    let mut ftyp = vec![0, 0, 0, 0x20];
    ftyp.extend_from_slice(b"ftypisom");
    assert_eq!(ext_from_magic(&ftyp), Some("mp4"));
    let mut heic = vec![0, 0, 0, 0x18];
    heic.extend_from_slice(b"ftypheic");
    assert_eq!(ext_from_magic(&heic), None);

    let it = RemoteItem {
        name: "Cutting Wood Building Material With a Circular Electric Saw".into(),
        video: true,
        url: "https://storage.coverr.co/videos/QatsCWWAorI71sZ33DkHZREGWruZCHsg/download?token=abc&filename=Cutting%20Wood".into(),
        credit: Some("Video from Coverr".into()),
        file_type: Some("mp4".into()),
        ..Default::default()
    };
    assert_eq!(ext_from_url(&it.url), "mp4");
    assert_eq!(
        it.library_file_stem(),
        "Cutting-Wood-Building-Material-With-a-Circular-Electric-Saw"
    );
    let dest = pick_library_dest(
        Path::new("/tmp"),
        &it.library_file_stem(),
        ext_from_url(&it.url),
        it.id.as_deref(),
    );
    assert_eq!(
        dest.file_name().and_then(|n| n.to_str()),
        Some("Cutting-Wood-Building-Material-With-a-Circular-Electric-Saw.mp4")
    );
    assert!(is_video(&dest), "{}", dest.display());
}

#[test]
fn pick_library_dest_does_not_double_ohr_prefix() {
    let dir = std::env::temp_dir().join(format!(
        "nwall-ohrdest-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let primary = dir.join("OHR.jpg");
    std::fs::write(&primary, b"one").unwrap();
    let other = RemoteItem {
        name: "other".into(),
        id: Some("OHR.OtherChina_EN-US1".into()),
        ..Default::default()
    };
    persist_remote_meta(&other, &primary);
    let dest = pick_library_dest(&dir, "OHR", "jpg", Some("OHR.KochiaChina_EN-US5037126636"));
    assert_eq!(
        dest.file_name().and_then(|n| n.to_str()),
        Some("OHR-KochiaChina_EN-US5037126636.jpg")
    );
    assert!(is_image(&dest), "{}", dest.display());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn persist_remote_meta_writes_and_keeps_sidecar_tags() {
    let dir = std::env::temp_dir().join(format!(
        "nwall-sidetags-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("sunset.jpg");
    std::fs::write(&dest, b"img").unwrap();
    let gh = RemoteItem {
        name: "sunset.jpg".into(),
        url: "https://raw.githubusercontent.com/o/r/sunset.jpg".into(),
        repo: Some("o/r".into()),
        id: Some("sunset.jpg".into()),
        tags: vec!["wallpaper".into(), "nature".into()],
        credit: Some("GitHub".into()),
        ..Default::default()
    };
    persist_remote_meta(&gh, &dest);
    let loaded = load_path_meta(&dest).expect("sidecar");
    assert_eq!(loaded.tags, vec!["wallpaper", "nature"]);
    assert_eq!(display_caption(Some(&loaded), "sunset.jpg"), "sunset.jpg");

    let empty = RemoteItem {
        name: "sunset.jpg".into(),
        url: gh.url.clone(),
        repo: Some("o/r".into()),
        id: Some("sunset.jpg".into()),
        credit: Some("GitHub".into()),
        ..Default::default()
    };
    persist_remote_meta(&empty, &dest);
    let kept = load_path_meta(&dest).expect("sidecar");
    assert_eq!(kept.tags, vec!["wallpaper", "nature"]);

    let pix = RemoteItem {
        name: "lake".into(),
        url: "https://pixabay.com/get/lake.jpg".into(),
        tags: vec!["lake".into(), "alps".into(), "snow".into()],
        credit: Some("Pixabay".into()),
        ..Default::default()
    };
    persist_remote_meta(&pix, &dest);
    let pix_loaded = load_path_meta(&dest).expect("sidecar");
    assert_eq!(pix_loaded.tags, vec!["lake", "alps", "snow"]);
    assert_eq!(
        display_caption(Some(&pix_loaded), "sunset.jpg"),
        "lake · alps · snow"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn discover_stats_item() -> RemoteItem {
    RemoteItem {
        name: "Aurora Loop".into(),
        video: true,
        url: "https://raw.githubusercontent.com/owner/repo/aurora.mp4".into(),
        credit: Some("GitHub".into()),
        id: Some("clips/aurora.mp4".into()),
        width: Some(1920),
        height: Some(1080),
        category: Some("nature".into()),
        file_size: Some(4_000_000),
        purity: Some("sfw".into()),
        uploader: Some("octocat".into()),
        avatar: Some("https://github.com/octocat.png?size=64".into()),
        date: Some("2024-01-15".into()),
        collection: Some("featured".into()),
        description: Some("Northern lights".into()),
        downloads: Some(12),
        views: Some(3400),
        favorites: Some(88),
        comments: Some(5),
        page_url: Some("https://github.com/owner/repo/blob/HEAD/clips/aurora.mp4".into()),
        colors: vec!["#112233".into(), "#aabbcc".into()],
        repo: Some("owner/repo".into()),
        license: Some("MIT".into()),
        tags: vec!["aurora".into(), "night".into()],
        duration_secs: Some(12.5),
        file_type: Some("mp4".into()),
        fps: Some(30.0),
        ..Default::default()
    }
}

fn assert_discover_stats(s: &MediaStats) {
    assert_eq!(s.width, Some(1920));
    assert_eq!(s.height, Some(1080));
    assert_eq!(s.file_size, Some(4_000_000));
    assert_eq!(s.duration_secs, Some(12.5));
    assert_eq!(s.fps, Some(30.0));
    assert_eq!(s.file_type.as_deref(), Some("mp4"));
    assert_eq!(s.source.as_deref(), Some("GitHub"));
    assert_eq!(s.title.as_deref(), Some("Aurora Loop"));
    assert_eq!(s.credit.as_deref(), Some("GitHub"));
    assert_eq!(s.repo.as_deref(), Some("owner/repo"));
    assert_eq!(s.source_id.as_deref(), Some("clips/aurora.mp4"));
    assert_eq!(
        s.page_url.as_deref(),
        Some("https://github.com/owner/repo/blob/HEAD/clips/aurora.mp4")
    );
    assert_eq!(s.category.as_deref(), Some("nature"));
    assert_eq!(s.purity.as_deref(), Some("sfw"));
    assert_eq!(s.views, Some(3400));
    assert_eq!(s.favorites, Some(88));
    assert_eq!(s.downloads, Some(12));
    assert_eq!(s.comments, Some(5));
    assert_eq!(s.date.as_deref(), Some("2024-01-15"));
    assert_eq!(s.collection.as_deref(), Some("featured"));
    assert_eq!(s.license.as_deref(), Some("MIT"));
    assert_eq!(s.description.as_deref(), Some("Northern lights"));
    assert_eq!(s.uploader.as_deref(), Some("octocat"));
    assert_eq!(
        s.avatar.as_deref(),
        Some("https://github.com/octocat.png?size=64")
    );
    assert_eq!(s.colors, vec!["#112233", "#aabbcc"]);
    assert_eq!(s.tags, vec!["aurora", "night"]);
}

#[test]
fn persist_remote_meta_keeps_all_discover_stats() {
    let dir = std::env::temp_dir().join(format!(
        "nwall-sidestats-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("aurora.mp4");
    std::fs::write(&dest, b"vid").unwrap();

    let item = discover_stats_item();
    assert_discover_stats(&MediaStats::from_remote(&item));

    persist_remote_meta(&item, &dest);
    assert_discover_stats(&load_path_meta(&dest).expect("sidecar"));

    let empty = RemoteItem {
        name: "Aurora Loop".into(),
        url: item.url.clone(),
        repo: Some("owner/repo".into()),
        id: Some("clips/aurora.mp4".into()),
        credit: Some("GitHub".into()),
        ..Default::default()
    };
    persist_remote_meta(&empty, &dest);
    assert_discover_stats(&load_path_meta(&dest).expect("kept remote"));

    persist_path_meta(
        &dest,
        &MediaStats {
            title: Some("Aurora Loop".into()),
            ..Default::default()
        },
    );
    let still = load_path_meta(&dest).expect("kept path");
    assert_discover_stats(&still);

    let mut with_music = still.clone();
    with_music.bg_music = Some(dir.join("track.mp3"));
    with_music.bg_music_volume = Some(0.4);
    with_music.bg_music_mute = Some(false);
    persist_path_meta(&dest, &with_music);
    let mus = load_path_meta(&dest).expect("music");
    assert_discover_stats(&mus);
    assert_eq!(
        mus.bg_music.as_deref(),
        Some(dir.join("track.mp3").as_path())
    );
    assert_eq!(mus.bg_music_volume, Some(0.4));

    let mut cleared = mus.clone();
    cleared.bg_music = None;
    cleared.bg_music_volume = None;
    cleared.bg_music_mute = None;
    persist_path_meta(&dest, &cleared);
    let after = load_path_meta(&dest).expect("cleared music");
    assert_discover_stats(&after);
    assert!(after.bg_music.is_none());
    assert!(after.bg_music_volume.is_none());
    assert!(after.bg_music_mute.is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn github_preview_complete_requires_tags() {
    let mut it = RemoteItem {
        name: "a.jpg".into(),
        url: "https://raw.githubusercontent.com/o/r/a.jpg".into(),
        repo: Some("o/r".into()),
        uploader: Some("octocat".into()),
        page_url: Some("https://github.com/o/r/blob/HEAD/a.jpg".into()),
        license: Some("MIT".into()),
        ..Default::default()
    };
    assert!(!github_preview_complete(&it));
    it.tags = vec!["wallpaper".into()];
    assert!(github_preview_complete(&it));
}

#[test]
fn wallhaven_save_enrich_requires_full_preview_fields() {
    let mut partial = RemoteItem {
        url: "https://w.wallhaven.cc/full/ab/wallhaven-abcdef.jpg".into(),
        id: Some("abcdef".into()),
        tags: vec!["nature".into()],
        avatar: Some("https://example.com/a.png".into()),
        uploader: Some("user".into()),
        // missing views / favorites / page_url / colors → not complete
        ..Default::default()
    };
    assert!(!wallhaven_preview_complete(&partial));
    partial.views = Some(1);
    partial.favorites = Some(2);
    partial.page_url = Some("https://whvn.cc/abcdef".into());
    partial.colors = vec!["#000000".into()];
    assert!(wallhaven_preview_complete(&partial));
}

#[test]
fn archive_tile_prefers_title_over_subjects() {
    let it = RemoteItem {
        name: "Aurora Over Iceland".into(),
        url: "https://archive.org/download/aurora-iceland".into(),
        credit: Some("Internet Archive".into()),
        id: Some("aurora-iceland".into()),
        tags: vec![
            "wallpaper".into(),
            "image".into(),
            "nature".into(),
            "desktop".into(),
        ],
        ..Default::default()
    };
    assert_eq!(it.tile_label(), "Aurora Over Iceland");
    assert_eq!(it.library_file_stem(), "aurora-iceland");
    let stats = it.stats();
    assert_eq!(stats.title.as_deref(), Some("Aurora Over Iceland"));
    assert_eq!(
        display_caption(Some(&stats), "aurora-iceland.jpg"),
        "Aurora Over Iceland"
    );
}

#[test]
fn archive_and_github_get_page_urls() {
    let mut arch = RemoteItem {
        id: Some("foo-bar".into()),
        url: "https://archive.org/download/foo-bar".into(),
        credit: Some("Internet Archive".into()),
        ..Default::default()
    };
    ensure_archive_page_url(&mut arch);
    assert_eq!(
        arch.page_url.as_deref(),
        Some("https://archive.org/details/foo-bar")
    );

    let mut gh = RemoteItem {
        repo: Some("owner/repo".into()),
        id: Some("path/file.mp4".into()),
        name: "file.mp4".into(),
        ..Default::default()
    };
    ensure_github_page_url(&mut gh);
    assert_eq!(
        gh.page_url.as_deref(),
        Some("https://github.com/owner/repo/blob/HEAD/path/file.mp4")
    );
}

#[test]
fn archive_subjects_become_tags_and_downloads_map_to_views() {
    let raw = serde_json::json!({
        "metadata": {
            "title": "dark-music-wallpaper",
            "creator": "kbmnj",
            "date": "2014-01-14",
            "mediatype": "image",
            "subject": ["image", "music", "desktop", "Wallpaper", "wallpaper"],
            "licenseurl": "http://creativecommons.org/licenses/by-nc-nd/3.0/",
            "collection": "opensource_image"
        },
        "item_size": 560380,
        "downloads": 34566,
        "files": [{
            "name": "dark-music-wallpaper.jpg",
            "size": "560380",
            "width": "1920",
            "height": "1080"
        }]
    });
    let d = archive_details_from_json(&raw);
    assert_eq!(d.views, Some(34566));
    assert_eq!(
        d.tags,
        vec!["image", "music", "desktop", "Wallpaper"] // case-insensitive dedupe
    );
    assert_eq!(d.license.as_deref(), Some("CC BY-NC-ND 3.0"));
    assert_eq!(d.width, Some(1920));
    assert_eq!(d.height, Some(1080));

    let mut item = RemoteItem {
        name: "dark-music-wallpaper".into(),
        url: "https://archive.org/download/dark-music-wallpaper".into(),
        credit: Some("Internet Archive".into()),
        id: Some("dark-music-wallpaper".into()),
        downloads: Some(999),
        ..Default::default()
    };
    item.apply_archive_details(&d);
    assert_eq!(item.views, Some(34566));
    assert!(item.downloads.is_none());
    assert_eq!(item.tags, d.tags);
    assert_eq!(item.license.as_deref(), Some("CC BY-NC-ND 3.0"));
    assert_eq!(item.uploader.as_deref(), Some("kbmnj"));
    // Subjects must not replace the IA metadata title on the tile / preview.
    assert_eq!(item.tile_label(), "dark-music-wallpaper");
    assert_eq!(
        remote_display_title(&item).as_deref(),
        Some("dark-music-wallpaper")
    );
    let stats = item.stats();
    assert_eq!(stats.title.as_deref(), Some("dark-music-wallpaper"));
    assert_eq!(
        display_caption(Some(&stats), "dark-music-wallpaper.jpg"),
        "dark-music-wallpaper"
    );

    let legacy = RemoteItem {
        url: "https://archive.org/download/x".into(),
        credit: Some("Internet Archive".into()),
        downloads: Some(42),
        ..Default::default()
    };
    let stats = MediaStats::from_remote(&legacy);
    assert_eq!(stats.views, Some(42));
    assert!(stats.downloads.is_none());
}

#[test]
fn archive_license_label_shortens_cc_urls() {
    assert_eq!(
        archive_license_label("https://creativecommons.org/licenses/by-nc/4.0/").as_deref(),
        Some("CC BY-NC 4.0")
    );
    assert_eq!(
        archive_license_label("http://creativecommons.org/publicdomain/mark/1.0/").as_deref(),
        Some("Public Domain Mark 1.0")
    );
    assert_eq!(
        archive_license_label("https://creativecommons.org/publicdomain/zero/1.0/").as_deref(),
        Some("CC0 1.0")
    );
}

#[test]
fn github_direct_child_and_space_encoding() {
    assert!(github_path_is_direct_child(
        "Live Wallpapers/giphy.gif",
        "Live Wallpapers"
    ));
    assert!(!github_path_is_direct_child(
        "Live Wallpapers/nested/x.gif",
        "Live Wallpapers"
    ));
    assert!(github_path_is_direct_child(
        "Cherry Blossoms/a.jpg",
        "Cherry Blossoms"
    ));
    assert!(github_path_is_direct_child("root.png", ""));
    assert!(!github_path_is_direct_child("dir/root.png", ""));
    assert!(github_path_is_direct_child(
        "animated/misc/a.mp4",
        "animated/misc"
    ));
    assert_eq!(
        percent_encode_path("Live Wallpapers/giphy.gif"),
        "Live%20Wallpapers/giphy.gif"
    );
    assert_eq!(percent_encode_path("Cherry Blossoms"), "Cherry%20Blossoms");
    assert!(github_size_looks_like_lfs_pointer(Some(132)));
    assert!(!github_size_looks_like_lfs_pointer(Some(1_000_000)));
    assert!(github_blob_download_url("o/r", "a/b.jpg", Some(132))
        .starts_with("https://media.githubusercontent.com/media/"));
    assert!(github_blob_download_url("o/r", "a/b.jpg", Some(5000))
        .starts_with("https://raw.githubusercontent.com/"));
}

#[test]
fn filter_github_repos_and_images_before_videos() {
    let items = vec![
        RemoteItem {
            name: "v.mp4".into(),
            video: true,
            url: "https://raw.githubusercontent.com/usman-369/wallpapers/HEAD/v.mp4".into(),
            repo: Some("usman-369/wallpapers".into()),
            ..Default::default()
        },
        RemoteItem {
            name: "a.jpg".into(),
            video: false,
            url: "https://raw.githubusercontent.com/usman-369/wallpapers/HEAD/a.jpg".into(),
            thumb: Some("https://raw.githubusercontent.com/usman-369/wallpapers/HEAD/a.jpg".into()),
            repo: Some("usman-369/wallpapers".into()),
            ..Default::default()
        },
        RemoteItem {
            name: "b.png".into(),
            video: false,
            url: "https://raw.githubusercontent.com/dharmx/walls/HEAD/b.png".into(),
            thumb: Some("https://raw.githubusercontent.com/dharmx/walls/HEAD/b.png".into()),
            repo: Some("dharmx/walls".into()),
            ..Default::default()
        },
    ];
    let mut sorted = items.clone();
    sorted.sort_by_key(|it| it.video);
    assert!(!sorted[0].video && !sorted[1].video && sorted[2].video);

    let only_usman = filter_by_github_repos(items, &["usman-369/wallpapers".into()]);
    assert_eq!(only_usman.len(), 2);
    assert!(only_usman
        .iter()
        .all(|i| { i.repo.as_deref() == Some("usman-369/wallpapers") }));
    assert!(filter_by_github_repos(only_usman.clone(), &[]).len() == 2);
}

#[test]
fn live_github_lists_items_including_spaced_paths() {
    let src = CatalogSource {
        name: "GitHub".into(),
        kind: "github".into(),
        url: String::new(),
        repo: String::new(),
        path: String::new(),
        repos: vec![
            nwall_ipc::GithubRepo {
                repo: "JaKooLit/Wallpaper-Bank".into(),
                path: "wallpapers".into(),
            },
            nwall_ipc::GithubRepo {
                repo: "FrenzyExists/wallpapers".into(),
                path: "Cherry Blossoms".into(),
            },
            nwall_ipc::GithubRepo {
                repo: "dharmx/walls".into(),
                path: "animated".into(),
            },
            nwall_ipc::GithubRepo {
                repo: "JoshuaThadi/Wall-E-Desk".into(),
                path: "Live Wallpapers".into(),
            },
        ],
        api_key: String::new(),
    };
    let items = github::fetch_github_source(&src).expect("github fetch");
    assert!(
        items.len() >= 10,
        "expected media from pack folders, got {}",
        items.len()
    );
    assert!(
        items.iter().any(|i| {
            i.id.as_deref()
                .is_some_and(|p| p.contains("Cherry Blossoms") || p.contains("Live Wallpapers"))
                || i.url.contains("Cherry%20Blossoms")
                || i.url.contains("Live%20Wallpapers")
        }),
        "missing spaced-path items"
    );
}

#[test]
fn live_github_builtin_pack_lists_items() {
    let mut sources = Vec::new();
    nwall_ipc::ensure_builtin_sources(&mut sources);
    let src = sources
        .into_iter()
        .find(|s| s.kind.eq_ignore_ascii_case("github") && s.name.eq_ignore_ascii_case("GitHub"))
        .expect("builtin GitHub source");
    assert!(
        src.repos.len() >= 50,
        "expected expanded pack, got {}",
        src.repos.len()
    );
    let items = github::fetch_github_source(&src).expect("github builtin pack fetch");
    eprintln!(
        "builtin GitHub pack: {} repos → {} items",
        src.repos.len(),
        items.len()
    );
    assert!(
        items.len() >= 100,
        "expected a large merged list, got {}",
        items.len()
    );
}

#[test]
fn infer_wallhaven_id_from_stem_and_page() {
    let path = Path::new("/tmp/dp19wl.jpg");
    let stats = MediaStats {
        credit: Some("Wallhaven".into()),
        purity: Some("sfw".into()),
        ..Default::default()
    };
    assert_eq!(infer_wallhaven_id(path, &stats).as_deref(), Some("dp19wl"));
    let with_page = MediaStats {
        page_url: Some("https://whvn.cc/e7kpl8".into()),
        ..Default::default()
    };
    assert_eq!(
        infer_wallhaven_id(Path::new("x.png"), &with_page).as_deref(),
        Some("e7kpl8")
    );
}

#[test]
fn bing_item_fills_preview_stats_fields() {
    assert_eq!(
        bing_format_startdate("20260821").as_deref(),
        Some("2026-08-21")
    );
    assert_eq!(
        bing_split_copyright("Winding road of Julier Pass, Switzerland (© Westend61/Getty Images)"),
        (
            Some("Winding road of Julier Pass, Switzerland".into()),
            "© Westend61/Getty Images".into()
        )
    );
    assert_eq!(
        bing_id_from_urlbase("/th?id=OHR.JulierPass_EN-US2643379571").as_deref(),
        Some("OHR.JulierPass_EN-US2643379571")
    );

    let it = bing_item(BingImage {
        title: "The climb is calling".into(),
        copyright: "Winding road of Julier Pass, Switzerland (© Westend61/Getty Images)".into(),
        copyrightlink: "https://www.bing.com/search?q=Julier+Pass+Switzerland&form=hpcapt".into(),
        startdate: "20260821".into(),
        urlbase: "/th?id=OHR.JulierPass_EN-US2643379571".into(),
        ..Default::default()
    })
    .expect("bing item");
    assert_eq!(it.name, "The climb is calling");
    assert_eq!(
        it.description.as_deref(),
        Some("Winding road of Julier Pass, Switzerland")
    );
    assert_eq!(it.credit.as_deref(), Some("© Westend61/Getty Images"));
    assert_eq!(it.date.as_deref(), Some("2026-08-21"));
    assert_eq!(
        it.page_url.as_deref(),
        Some("https://www.bing.com/search?q=Julier+Pass+Switzerland&form=hpcapt")
    );
    assert_eq!(it.id.as_deref(), Some("OHR.JulierPass_EN-US2643379571"));
    assert!(it.url.ends_with("_UHD.jpg"), "{}", it.url);
    assert_eq!(it.file_type.as_deref(), Some("JPEG"));
    assert_eq!(ext_from_url(&it.url), "jpg");
    assert_eq!(it.library_file_stem(), "OHR-JulierPass_EN-US2643379571");
    let dest = pick_library_dest(
        Path::new("/tmp"),
        &it.library_file_stem(),
        ext_from_url(&it.url),
        it.id.as_deref(),
    );
    assert_eq!(
        dest.file_name().and_then(|n| n.to_str()),
        Some("OHR-JulierPass_EN-US2643379571.jpg")
    );
    assert!(is_image(&dest), "{}", dest.display());

    let stats = it.stats();
    assert_eq!(stats.source.as_deref(), Some("Bing Daily"));
    assert_eq!(stats.title.as_deref(), Some("The climb is calling"));
    assert_eq!(stats.date.as_deref(), Some("2026-08-21"));
    assert!(stats.page_url.is_some());
    assert!(stats.description.is_some());
}

#[test]
fn nasa_item_fills_preview_stats_fields() {
    assert_eq!(
        nasa_apod_page_url("2026-08-21").as_deref(),
        Some("https://apod.nasa.gov/apod/ap260821.html")
    );
    assert_eq!(
        nasa_file_type_from_url("https://apod.nasa.gov/apod/image/2608/example_hd.jpg?foo=1")
            .as_deref(),
        Some("JPEG")
    );
    assert_eq!(
        nasa_youtube_watch_url("https://www.youtube.com/embed/1R5QqhPq1Ik?rel=0").as_deref(),
        Some("https://www.youtube.com/watch?v=1R5QqhPq1Ik")
    );

    let it = nasa_item(NasaApod {
        title: "Filaments of the Vela Supernova Remnant".into(),
        explanation: Some("About eleven thousand years ago…".into()),
        date: Some("2014-10-01".into()),
        url: "https://apod.nasa.gov/apod/image/1410/vela_960.jpg".into(),
        hdurl: Some("https://apod.nasa.gov/apod/image/1410/vela_2000.jpg".into()),
        media_type: "image".into(),
        copyright: Some("Jade Scope".into()),
        ..Default::default()
    })
    .expect("nasa item");
    assert_eq!(it.name, "Filaments of the Vela Supernova Remnant");
    assert_eq!(it.credit.as_deref(), Some("NASA / Jade Scope"));
    assert_eq!(it.date.as_deref(), Some("2014-10-01"));
    assert_eq!(it.id.as_deref(), Some("2014-10-01"));
    assert_eq!(
        it.description.as_deref(),
        Some("About eleven thousand years ago…")
    );
    assert_eq!(
        it.page_url.as_deref(),
        Some("https://apod.nasa.gov/apod/ap141001.html")
    );
    assert_eq!(
        it.url,
        "https://apod.nasa.gov/apod/image/1410/vela_2000.jpg"
    );
    assert_eq!(
        it.thumb.as_deref(),
        Some("https://apod.nasa.gov/apod/image/1410/vela_960.jpg")
    );
    assert_eq!(it.file_type.as_deref(), Some("JPEG"));

    let stats = it.stats();
    assert_eq!(stats.source.as_deref(), Some("NASA APOD"));
    assert_eq!(
        stats.title.as_deref(),
        Some("Filaments of the Vela Supernova Remnant")
    );
    assert_eq!(stats.date.as_deref(), Some("2014-10-01"));
    assert!(stats.page_url.is_some());
    assert!(stats.description.is_some());
    assert_eq!(stats.credit.as_deref(), Some("NASA / Jade Scope"));
    assert_eq!(stats.file_type.as_deref(), Some("JPEG"));

    // YouTube APODs are not wallpaper downloads.
    assert!(nasa_item(NasaApod {
        title: "Earthrise".into(),
        media_type: "video".into(),
        date: Some("2018-12-23".into()),
        url: "https://www.youtube.com/embed/1R5QqhPq1Ik?rel=0".into(),
        explanation: Some("About 12 seconds…".into()),
        ..Default::default()
    })
    .is_none());
}

#[test]
fn search_cache_rejects_incomplete_bing_nasa() {
    let stale_bing = FetchResult::unpaged(vec![RemoteItem {
        name: "The climb is calling".into(),
        url: "https://www.bing.com/th?id=OHR.JulierPass_UHD.jpg".into(),
        credit: Some("Winding road of Julier Pass, Switzerland (© Westend61/Getty Images)".into()),
        ..Default::default()
    }]);
    assert!(cache_has_incomplete_daily_meta(&stale_bing));

    let fresh = FetchResult::unpaged(vec![bing_item(BingImage {
        title: "The climb is calling".into(),
        copyright: "Winding road of Julier Pass, Switzerland (© Westend61/Getty Images)".into(),
        copyrightlink: "https://www.bing.com/search?q=Julier".into(),
        startdate: "20260821".into(),
        urlbase: "/th?id=OHR.JulierPass_EN-US2643379571".into(),
        ..Default::default()
    })
    .expect("bing")]);
    assert!(!cache_has_incomplete_daily_meta(&fresh));

    let stale_nasa = FetchResult::unpaged(vec![RemoteItem {
        name: "Eclipse".into(),
        url: "https://apod.nasa.gov/apod/image/2608/x.jpeg".into(),
        credit: Some("NASA / Someone".into()),
        ..Default::default()
    }]);
    assert!(cache_has_incomplete_daily_meta(&stale_nasa));
}

fn prefetch_src(kind: &str, tag: &str) -> CatalogSource {
    CatalogSource {
        name: format!("prefetch-{tag}"),
        kind: kind.into(),
        url: format!("https://example.test/{tag}"),
        repo: String::new(),
        path: String::new(),
        repos: Vec::new(),
        api_key: String::new(),
    }
}

#[test]
fn prefetchable_next_page_skips_unpaged_and_random() {
    let opts = SearchOpts {
        page: 1,
        ..Default::default()
    };
    for kind in ["bing", "nasa", "index"] {
        assert!(
            prefetchable_next_page(kind, &opts, Some(1)).is_none(),
            "{kind}"
        );
    }
    let random = SearchOpts {
        page: 1,
        sorting: "random".into(),
        ..Default::default()
    };
    assert!(prefetchable_next_page("wallhaven", &random, None).is_none());
}

#[test]
fn prefetchable_next_page_advances_until_last() {
    let opts = SearchOpts {
        page: 2,
        query: "mountains".into(),
        ..Default::default()
    };
    let next = prefetchable_next_page("pixabay", &opts, Some(10)).expect("next");
    assert_eq!(next.page, 3);
    assert_eq!(next.query, "mountains");
    let at_end = SearchOpts {
        page: 5,
        ..Default::default()
    };
    assert!(prefetchable_next_page("coverr", &at_end, Some(5)).is_none());
    assert!(prefetchable_next_page("archive", &opts, None).is_some());
    assert!(prefetchable_next_page("github", &opts, Some(4)).is_some());
}

#[test]
fn listing_thumb_urls_prefers_thumb_skips_bare_video() {
    let items = [
        RemoteItem {
            thumb: Some("https://t.example/1.jpg".into()),
            url: "https://full.example/1.jpg".into(),
            video: false,
            ..Default::default()
        },
        RemoteItem {
            thumb: None,
            url: "https://full.example/2.jpg".into(),
            video: false,
            ..Default::default()
        },
        RemoteItem {
            thumb: None,
            url: "https://full.example/3.mp4".into(),
            video: true,
            ..Default::default()
        },
        RemoteItem {
            thumb: Some("https://t.example/4.jpg".into()),
            url: "https://full.example/4.mp4".into(),
            video: true,
            ..Default::default()
        },
    ];
    assert_eq!(
        listing_thumb_urls(&items),
        vec![
            "https://t.example/1.jpg",
            "https://full.example/2.jpg",
            "https://t.example/4.jpg"
        ]
    );
}

#[test]
fn peek_cached_fetch_remembers_shown_and_next_page() {
    let src = prefetch_src("wallhaven", "mem-shown-next");
    let page1 = SearchOpts {
        page: 1,
        query: "prefetch-mem-shown-next".into(),
        ..Default::default()
    };
    let page2 = SearchOpts {
        page: 2,
        query: "prefetch-mem-shown-next".into(),
        ..Default::default()
    };
    let shown = FetchResult {
        items: vec![RemoteItem {
            name: "one".into(),
            url: "https://example.test/one.jpg".into(),
            ..Default::default()
        }],
        page: 1,
        last_page: Some(4),
    };
    let next = FetchResult {
        items: vec![RemoteItem {
            name: "two".into(),
            url: "https://example.test/two.jpg".into(),
            ..Default::default()
        }],
        page: 2,
        last_page: Some(4),
    };
    remember_shown_fetch(&src, &page1, &shown);
    remember_fetch(&src, &page2, &next);
    let got1 = peek_cached_fetch(&src, &page1).expect("page1");
    let got2 = peek_cached_fetch(&src, &page2).expect("page2");
    assert_eq!(got1.page, 1);
    assert_eq!(got1.items[0].name, "one");
    assert_eq!(got2.page, 2);
    assert_eq!(got2.items[0].name, "two");
    let miss = SearchOpts {
        page: 1,
        query: "prefetch-mem-shown-next-other".into(),
        ..Default::default()
    };
    assert!(peek_cached_fetch(&src, &miss).is_none());
}

#[test]
fn parse_retry_after_reads_seconds_and_clamps() {
    use std::time::Duration;
    assert_eq!(parse_retry_after(None), None);
    assert_eq!(parse_retry_after(Some("")), None);
    assert_eq!(parse_retry_after(Some("  ")), None);
    assert_eq!(parse_retry_after(Some("5")), Some(Duration::from_secs(5)));
    assert_eq!(parse_retry_after(Some("0")), Some(Duration::from_secs(1)));
    assert_eq!(parse_retry_after(Some("99")), Some(Duration::from_secs(30)));
    assert_eq!(
        parse_retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT")),
        None
    );
}

#[test]
fn source_uses_auth_budget_key_vs_anon() {
    let mut src = prefetch_src("wallhaven", "auth-budget");
    src.api_key = "wh-key".into();
    assert!(source_uses_auth_budget(&src));

    let mut gh = prefetch_src("github", "auth-budget");
    gh.api_key = "ghp_test".into();
    assert!(source_uses_auth_budget(&gh));

    let mut nasa = prefetch_src("nasa", "auth-budget");
    nasa.api_key = "DEMO_KEY".into();
    assert!(!source_uses_auth_budget(&nasa));
    nasa.api_key = "real-nasa-key".into();
    assert!(source_uses_auth_budget(&nasa));

    let mut pix = prefetch_src("pixabay", "auth-budget");
    pix.api_key = "px".into();
    assert!(source_uses_auth_budget(&pix));
}

#[test]
fn wallhaven_rate_limit_message_is_short_and_unique() {
    let msg = WALLHAVEN_RATE_LIMIT_MSG;
    assert_eq!(msg, "Wallhaven rate limit, try again in a moment");
    assert!(!msg.contains("http"));
    assert!(!msg.contains("://"));
    assert!(http_err_is_rate_limit(&anyhow::anyhow!("{msg}")));
    assert!(http_err_is_rate_limit(&anyhow::anyhow!("status code 429")));
    assert!(!http_err_is_rate_limit(&anyhow::anyhow!(
        "wallhaven HTTP 500"
    )));
}
