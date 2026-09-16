mod bg_music;
mod discover;
mod gallery;
mod monitors;
mod preview;
mod settings;
mod stats;

use std::cell::{Cell, RefCell};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adw::prelude::*;
use gtk::{
    gdk, gio, glib, Align, Box as GtkBox, Button, DropDown, Entry, FileDialog, FlowBox, HeaderBar,
    Image, Label, MenuButton, Orientation, Picture, PolicyType, Popover, ScrolledWindow, Separator,
    SpinButton, Stack, StackSwitcher, StringList, Switch,
};
use nwall_catalog as catalog;
use nwall_ipc::{
    client_request, config_dir, default_config_path, is_audio, is_video,
    normalize_preview_width_pct, Config, FitMode, Request,
};

use crate::app::{fire_source_watchers, watch_sources, SourceWatchers};
use crate::consts::*;
use crate::ipc_util::{apply_wallpaper, ipc_ok, push_playback};
use crate::theme::{
    apply_theme, bind_window_zoom, install_app_icon, load_builtin_css, load_css,
    lock_tile_selection_chrome,
};
use crate::widgets::{preview_option_row, set_video_playback_rows_visible, spin_with_percent};

use self::bg_music::{open_archive_music_dialog, open_yt_music_dialog};
use self::discover::build_discover;
use self::gallery::{
    child_path, compact_flow, empty_gallery_page, filter_gallery, gallery_dirs, refresh_gallery,
    refresh_status,
};
use self::monitors::{fill_monitor_bar, refresh_monitor_mocks};
use self::preview::{
    apply_sidebar_pct, bind_preview_host_size, bind_preview_split, bind_preview_visibility_pause,
    debounce_persist_interface, fill_sidebar_button, make_fixed_preview, make_preview_split,
    new_preview_sidebar, persist_interface, persist_show_monitors, preview_caption_label,
    preview_section_label, preview_sidebar_head, preview_stats_scroll, preview_title,
    refresh_bg_music_ui, set_preview_title, show_preview, stop_live_preview, PreviewLoading,
    PreviewSession,
};
use self::settings::build_settings;
use self::stats::{make_stats_pane, schedule_local_stats, StatsPane};

pub(crate) fn build(app: &adw::Application) {
    let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
    apply_theme(app, &cfg);

    load_builtin_css();
    if let Some(css_path) = cfg.theme_css.clone().or_else(|| {
        let p = config_dir().join("theme.css");
        p.exists().then_some(p)
    }) {
        load_css(&css_path);
    }
    lock_tile_selection_chrome();

    install_app_icon();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("nwall")
        .icon_name(APP_ICON_NAME)
        .default_width(1100)
        .default_height(700)
        .build();
    gtk::Window::set_default_icon_name(APP_ICON_NAME);
    window.set_icon_name(Some(APP_ICON_NAME));

    let header = HeaderBar::new();

    let stack = Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    let switcher = StackSwitcher::new();
    switcher.set_stack(Some(&stack));
    header.set_title_widget(Some(&switcher));

    let flow = FlowBox::new();
    compact_flow(&flow);

    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Automatic)
        .vscrollbar_policy(PolicyType::Automatic)
        .child(&flow)
        .hexpand(true)
        .vexpand(true)
        .build();
    let gallery_empty = empty_gallery_page("No wallpapers found — add files or use Add wallpaper");
    let gallery_body = Stack::new();
    gallery_body.set_hexpand(true);
    gallery_body.set_vexpand(true);
    gallery_body.add_named(&scroll, Some("grid"));
    gallery_body.add_named(&gallery_empty, Some("empty"));
    gallery_body.set_visible_child_name("grid");

    let filter_list = StringList::new(&["All", "Images", "Videos"]);
    let filter = DropDown::new(Some(filter_list), Option::<&gtk::Expression>::None);
    filter.set_selected(0);

    let monitor_bar = GtkBox::new(Orientation::Horizontal, 8);
    monitor_bar.set_margin_top(6);
    monitor_bar.set_margin_bottom(4);
    monitor_bar.set_margin_start(12);
    monitor_bar.set_margin_end(12);
    monitor_bar.set_halign(Align::Center);
    monitor_bar.set_valign(Align::Center);
    let (selected_monitors, monitor_mocks) = fill_monitor_bar(&monitor_bar);
    let sync_mocks = {
        let mocks = Arc::clone(&monitor_mocks);
        Arc::new(move || {
            refresh_monitor_mocks(&mocks);
        }) as Arc<dyn Fn() + Send + Sync>
    };
    let monitor_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Automatic)
        .vscrollbar_policy(PolicyType::Never)
        .child(&monitor_bar)
        .hexpand(true)
        .build();
    monitor_scroll.set_min_content_height(118);
    monitor_scroll.set_max_content_height(260);
    monitor_scroll.set_propagate_natural_height(true);

    let monitors_expanded = Rc::new(Cell::new(cfg.show_monitors));
    let monitors_chevron = Image::from_icon_name(if cfg.show_monitors {
        "pan-down-symbolic"
    } else {
        "pan-end-symbolic"
    });
    monitors_chevron.set_pixel_size(12);
    let monitors_toggle_lbl = Label::new(Some("Monitors"));
    let monitors_toggle_row = GtkBox::new(Orientation::Horizontal, 6);
    monitors_toggle_row.set_halign(Align::Center);
    monitors_toggle_row.append(&monitors_toggle_lbl);
    monitors_toggle_row.append(&monitors_chevron);
    let monitors_toggle = Button::new();
    monitors_toggle.add_css_class("monitors-collapse");
    monitors_toggle.set_halign(Align::Center);
    monitors_toggle.set_tooltip_text(Some(if cfg.show_monitors {
        "Hide monitors"
    } else {
        "Show monitors"
    }));
    monitors_toggle.set_child(Some(&monitors_toggle_row));
    monitor_scroll.set_visible(cfg.show_monitors);
    {
        let scroll = monitor_scroll.clone();
        let expanded = Rc::clone(&monitors_expanded);
        let chevron = monitors_chevron.clone();
        let btn = monitors_toggle.clone();
        monitors_toggle.connect_clicked(move |_| {
            let next = !expanded.get();
            expanded.set(next);
            scroll.set_visible(next);
            chevron.set_icon_name(Some(if next {
                "pan-down-symbolic"
            } else {
                "pan-end-symbolic"
            }));
            btn.set_tooltip_text(Some(if next {
                "Hide monitors"
            } else {
                "Show monitors"
            }));
            persist_show_monitors(next);
        });
    }

    let monitors_sep = Separator::new(Orientation::Horizontal);
    monitors_sep.add_css_class("monitors-section-sep");

    let monitors_section = GtkBox::new(Orientation::Vertical, 0);
    monitors_section.set_margin_top(8);
    monitors_toggle.set_margin_bottom(4);
    monitors_section.append(&monitors_toggle);
    monitors_section.append(&monitor_scroll);
    monitors_section.append(&monitors_sep);

    let filter_bar = GtkBox::new(Orientation::Horizontal, 8);
    filter_bar.set_margin_top(6);
    filter_bar.set_margin_start(12);
    filter_bar.set_margin_end(12);
    filter_bar.append(&Label::new(Some("Show:")));
    filter_bar.append(&filter);
    let add_wallpaper_split = GtkBox::new(Orientation::Horizontal, 0);
    add_wallpaper_split.add_css_class("linked");
    let add_wallpaper_btn = Button::with_label("Add wallpaper");
    add_wallpaper_btn.set_tooltip_text(Some("Choose an image or video…"));
    let add_youtube_btn = Button::with_label("Add from YouTube");
    add_youtube_btn.add_css_class("flat");
    add_youtube_btn.set_halign(Align::Fill);
    let add_more_popover = Popover::new();
    add_more_popover.set_child(Some(&add_youtube_btn));
    let add_more_btn = MenuButton::new();
    add_more_btn.set_icon_name("pan-down-symbolic");
    add_more_btn.set_tooltip_text(Some("More add options"));
    add_more_btn.set_popover(Some(&add_more_popover));
    add_wallpaper_split.append(&add_wallpaper_btn);
    add_wallpaper_split.append(&add_more_btn);
    filter_bar.append(&add_wallpaper_split);
    let library_search = Entry::new();
    library_search.set_hexpand(true);
    library_search.set_placeholder_text(Some("Search…"));
    library_search.set_tooltip_text(Some("Filter by filename or title"));
    filter_bar.append(&library_search);

    let (preview_host, preview_pic, preview_loading) = make_fixed_preview();

    let preview_name = preview_title("Click a wallpaper");
    let preview_meta = preview_caption_label("Select monitors above, then Apply.", false);

    let apply_btn = Button::with_label("Apply");
    apply_btn.add_css_class("suggested-action");
    apply_btn.set_sensitive(false);
    fill_sidebar_button(&apply_btn);

    let delete_btn = Button::with_label("Delete");
    delete_btn.add_css_class("destructive-action");
    delete_btn.set_sensitive(false);
    fill_sidebar_button(&delete_btn);

    let music_section = GtkBox::new(Orientation::Vertical, 6);
    music_section.add_css_class("bg-music-section");
    music_section.set_margin_top(4);

    let music_heading = Label::new(Some("Background music"));
    music_heading.set_halign(Align::Start);
    music_heading.set_hexpand(true);
    music_heading.add_css_class("heading");
    music_heading.set_tooltip_text(Some(
        "Looping track for this wallpaper — pick a local file, Archive.org, or YouTube Music",
    ));

    let music_actions = GtkBox::new(Orientation::Horizontal, 6);
    music_actions.set_hexpand(true);
    music_actions.set_homogeneous(true);
    let music_file_btn = Button::with_label("File");
    music_file_btn.set_sensitive(false);
    music_file_btn.set_tooltip_text(Some("Choose an mp3, ogg, flac, m4a, wav, …"));
    fill_sidebar_button(&music_file_btn);
    let music_archive_btn = Button::with_label("Archive");
    music_archive_btn.set_sensitive(false);
    music_archive_btn.set_tooltip_text(Some("Search Internet Archive for background music"));
    fill_sidebar_button(&music_archive_btn);
    let music_yt_btn = Button::with_label("YT Music");
    music_yt_btn.set_sensitive(false);
    music_yt_btn.set_tooltip_text(Some("Search YouTube Music"));
    fill_sidebar_button(&music_yt_btn);
    music_actions.append(&music_file_btn);
    music_actions.append(&music_archive_btn);
    music_actions.append(&music_yt_btn);

    let remove_music_btn = Button::with_label("Remove");
    remove_music_btn.set_visible(false);
    fill_sidebar_button(&remove_music_btn);

    let music_name = preview_caption_label("", false);
    music_name.set_visible(false);
    music_name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    music_name.set_halign(Align::Start);
    music_name.set_justify(gtk::Justification::Left);
    music_name.set_xalign(0.0);

    let bg_mute = Switch::new();
    bg_mute.set_halign(Align::End);
    let bg_vol = SpinButton::with_range(0.0, 100.0, 1.0);
    bg_vol.set_digits(0);
    bg_vol.set_value(50.0);
    let bg_mute_row = preview_option_row("Mute", &bg_mute);
    let bg_vol_row = preview_option_row("Volume", spin_with_percent(&bg_vol));
    bg_mute_row.set_visible(false);
    bg_vol_row.set_visible(false);
    let suppress_bg_music = Rc::new(Cell::new(false));

    music_section.append(&music_heading);
    music_section.append(&music_actions);
    music_section.append(&remove_music_btn);
    music_section.append(&music_name);
    music_section.append(&bg_mute_row);
    music_section.append(&bg_vol_row);

    let apply_fps = SpinButton::with_range(0.0, 240.0, 1.0);
    apply_fps.set_value(cfg.fps as f64);
    apply_fps.set_tooltip_text(Some("0 = freeze video as a still (no decoder)"));
    let apply_mute = Switch::new();
    apply_mute.set_active(cfg.mute);
    apply_mute.set_halign(Align::End);
    let apply_vol = SpinButton::with_range(0.0, 100.0, 1.0);
    apply_vol.set_digits(0);
    apply_vol.set_value((cfg.volume * 100.0) as f64);
    let apply_fit_list = StringList::new(&["Cover", "Contain", "Stretch"]);
    let apply_fit = DropDown::new(Some(apply_fit_list), Option::<&gtk::Expression>::None);
    apply_fit.set_selected(match cfg.fit {
        FitMode::Cover => 0,
        FitMode::Contain => 1,
        FitMode::Stretch => 2,
    });
    let fps_row = preview_option_row("FPS", &apply_fps);
    let mute_row = preview_option_row("Mute", &apply_mute);
    let vol_row = preview_option_row("Volume", spin_with_percent(&apply_vol));
    set_video_playback_rows_visible(&fps_row, &mute_row, &vol_row, false);

    let preview_stats = make_stats_pane();
    let preview_box = new_preview_sidebar();
    let head = preview_sidebar_head();
    head.append(&preview_section_label());
    head.append(&preview_host);
    head.append(&preview_name);
    head.append(&preview_meta);
    head.append(&fps_row);
    head.append(&mute_row);
    head.append(&vol_row);
    head.append(&preview_option_row("Fit", &apply_fit));
    head.append(&apply_btn);
    head.append(&delete_btn);
    head.append(&music_section);
    preview_box.append(&head);
    let gallery_stats_scroll = preview_stats_scroll(&preview_stats.root);
    gallery_stats_scroll.set_visible(cfg.show_stats);
    preview_box.append(&gallery_stats_scroll);

    let preview_width = Rc::new(Cell::new(normalize_preview_width_pct(cfg.preview_width)));
    let show_stats_on = Rc::new(Cell::new(cfg.show_stats));
    let gui_zoom = Rc::new(Cell::new(cfg.gui_zoom));
    let paned_suppress = Rc::new(Cell::new(false));
    let persist_tick = Rc::new(Cell::new(0u64));
    bind_preview_host_size(&preview_box, &preview_host);
    let split = make_preview_split(&gallery_body, &preview_box, preview_width.get());

    let status = Label::new(None);
    status.set_halign(Align::Start);
    status.set_margin_start(12);
    status.set_margin_end(12);
    status.set_margin_bottom(8);
    status.add_css_class("dim-label");
    status.set_ellipsize(gtk::pango::EllipsizeMode::Middle);

    let gallery_box = GtkBox::new(Orientation::Vertical, 0);
    gallery_box.append(&monitors_section);
    gallery_box.append(&filter_bar);
    gallery_box.append(&split);
    gallery_box.append(&status);
    stack.add_titled(&gallery_box, Some("gallery"), "Wallpapers");

    let source_watchers: SourceWatchers = Rc::new(RefCell::new(Vec::new()));
    let settings = build_settings(&mut cfg, Rc::clone(&source_watchers));

    let root = GtkBox::new(Orientation::Vertical, 0);
    root.append(&header);
    root.append(&stack);
    window.set_content(Some(&root));

    bind_window_zoom(&window, Rc::clone(&gui_zoom), &settings.gui_zoom);

    let selected: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let live_preview: Arc<Mutex<Option<PreviewSession>>> = Arc::new(Mutex::new(None));
    let preview_gen: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let dirs = gallery_dirs(&cfg.library);
    let (discover, discover_split, discover_stats_scroll, discover_first_load) = build_discover(
        &cfg,
        Arc::clone(&selected),
        Rc::clone(&selected_monitors),
        gallery_body.clone(),
        flow.clone(),
        dirs.clone(),
        filter.clone(),
        library_search.clone(),
        Arc::clone(&live_preview),
        Arc::clone(&preview_gen),
        Rc::clone(&source_watchers),
        Arc::clone(&sync_mocks),
        preview_width.get(),
        show_stats_on.get(),
    );
    stack.add_titled(&discover, Some("discover"), "Discover");
    stack.add_titled(&settings.root, Some("settings"), "Settings");
    bind_preview_split(
        &split,
        &discover_split,
        &preview_width,
        &paned_suppress,
        &settings.preview_width,
        &show_stats_on,
        &gui_zoom,
        &persist_tick,
    );
    bind_preview_split(
        &discover_split,
        &split,
        &preview_width,
        &paned_suppress,
        &settings.preview_width,
        &show_stats_on,
        &gui_zoom,
        &persist_tick,
    );
    {
        let show = Rc::clone(&show_stats_on);
        let g_stats = gallery_stats_scroll.clone();
        let d_stats = discover_stats_scroll.clone();
        let width = Rc::clone(&preview_width);
        let zoom = Rc::clone(&gui_zoom);
        settings.show_stats.connect_active_notify(move |sw| {
            let on = sw.is_active();
            show.set(on);
            g_stats.set_visible(on);
            d_stats.set_visible(on);
            persist_interface(on, width.get(), zoom.get());
        });
    }
    {
        settings
            .fast_image_preview
            .connect_active_notify(move |sw| {
                let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
                cfg.fast_image_preview = sw.is_active();
                let _ = cfg.save(&default_config_path());
            });
        settings
            .fast_video_preview
            .connect_active_notify(move |sw| {
                let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
                cfg.fast_video_preview = sw.is_active();
                let _ = cfg.save(&default_config_path());
            });
    }
    {
        let apply_library = {
            let body = gallery_body.clone();
            let flow = flow.clone();
            let filter = filter.clone();
            let search = library_search.clone();
            let entry = settings.library_entry.clone();
            move |raw: String| {
                let path = expand_library_path(raw.trim());
                if path.as_os_str().is_empty() {
                    return;
                }
                let _ = std::fs::create_dir_all(&path);
                let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
                cfg.library = path.clone();
                let _ = cfg.save(&default_config_path());
                entry.set_text(&path.display().to_string());
                let dirs = gallery_dirs(&path);
                refresh_gallery(&body, &flow, &dirs, filter.selected(), &search.text());
            }
        };

        let apply_activate = apply_library.clone();
        settings.library_entry.connect_activate(move |entry| {
            apply_activate(entry.text().to_string());
        });

        let win = window.clone();
        let entry = settings.library_entry.clone();
        let apply_browse = apply_library;
        settings.library_browse.connect_clicked(move |_| {
            let dialog = FileDialog::builder()
                .title("Wallpaper library folder")
                .modal(true)
                .build();
            let current = expand_library_path(entry.text().trim());
            if current.is_dir() {
                dialog.set_initial_folder(Some(&gio::File::for_path(&current)));
            }
            let entry = entry.clone();
            let apply = apply_browse.clone();
            dialog.select_folder(Some(&win), gio::Cancellable::NONE, move |res| {
                let Ok(file) = res else {
                    return;
                };
                let Some(path) = file.path() else {
                    return;
                };
                entry.set_text(&path.display().to_string());
                apply(path.display().to_string());
            });
        });
    }
    {
        let width = Rc::clone(&preview_width);
        let suppress = Rc::clone(&paned_suppress);
        let g_split = split.clone();
        let d_split = discover_split.clone();
        let show = Rc::clone(&show_stats_on);
        let zoom = Rc::clone(&gui_zoom);
        let tick = Rc::clone(&persist_tick);
        settings.preview_width.connect_value_changed(move |s| {
            if suppress.get() {
                return;
            }
            let pct = normalize_preview_width_pct(s.value());
            width.set(pct);
            suppress.set(true);
            apply_sidebar_pct(&g_split, pct);
            apply_sidebar_pct(&d_split, pct);
            suppress.set(false);
            debounce_persist_interface(&show, &width, &zoom, &tick);
        });
    }
    let watchers_tab = Rc::clone(&source_watchers);
    let live_tab = Arc::clone(&live_preview);
    let g_split_tab = split.clone();
    let d_split_tab = discover_split.clone();
    let width_tab = Rc::clone(&preview_width);
    let suppress_tab = Rc::clone(&paned_suppress);
    stack.connect_visible_child_notify(move |stack| {
        stop_live_preview(&live_tab);
        if stack.visible_child_name().as_deref() == Some("discover") {
            discover_first_load();
            fire_source_watchers(&watchers_tab);
        }
        let w = width_tab.get();
        let target = match stack.visible_child_name().as_deref() {
            Some("gallery") => Some(g_split_tab.clone()),
            Some("discover") => Some(d_split_tab.clone()),
            _ => None,
        };
        if let Some(p) = target {
            let suppress = Rc::clone(&suppress_tab);
            let tries = Rc::new(Cell::new(0u8));
            glib::idle_add_local(move || {
                if p.width() <= 0 {
                    let n = tries.get().saturating_add(1);
                    tries.set(n);
                    return if n < 30 {
                        glib::ControlFlow::Continue
                    } else {
                        glib::ControlFlow::Break
                    };
                }
                suppress.set(true);
                apply_sidebar_pct(&p, w);
                suppress.set(false);
                glib::ControlFlow::Break
            });
        }
    });
    refresh_gallery(&gallery_body, &flow, &dirs, 0, "");
    refresh_status(&status);
    refresh_monitor_mocks(&monitor_mocks);

    let body_r = gallery_body.clone();
    let flow_r = flow.clone();
    let search_r = library_search.clone();
    filter.connect_selected_notify(move |dd| {
        filter_gallery(&body_r, &flow_r, dd.selected(), &search_r.text());
    });
    let body_s = gallery_body.clone();
    let flow_s = flow.clone();
    let filter_s = filter.clone();
    let search_tick = Rc::new(Cell::new(0u64));
    library_search.connect_changed(move |entry| {
        let n = search_tick.get() + 1;
        search_tick.set(n);
        let body = body_s.clone();
        let flow = flow_s.clone();
        let filter = filter_s.clone();
        let entry = entry.clone();
        let tick = Rc::clone(&search_tick);
        glib::timeout_add_local(Duration::from_millis(150), move || {
            if tick.get() != n {
                return glib::ControlFlow::Break;
            }
            filter_gallery(&body, &flow, filter.selected(), &entry.text());
            glib::ControlFlow::Break
        });
    });

    let selected_s = Arc::clone(&selected);
    let preview_pic_s = preview_pic.clone();
    let preview_name_s = preview_name.clone();
    let preview_meta_s = preview_meta.clone();
    let preview_loading_s = preview_loading.clone();
    let preview_stats_s = preview_stats.clone();
    let apply_s = apply_btn.clone();
    let delete_s = delete_btn.clone();
    let music_btn_s = music_file_btn.clone();
    let music_archive_s = music_archive_btn.clone();
    let music_yt_s = music_yt_btn.clone();
    let remove_music_s = remove_music_btn.clone();
    let music_name_s = music_name.clone();
    let bg_mute_s = bg_mute.clone();
    let bg_vol_s = bg_vol.clone();
    let bg_mute_row_s = bg_mute_row.clone();
    let bg_vol_row_s = bg_vol_row.clone();
    let suppress_bg_s = Rc::clone(&suppress_bg_music);
    let live_s = Arc::clone(&live_preview);
    let gen_s = Arc::clone(&preview_gen);
    let stats_gen_s = Arc::new(AtomicU64::new(0));
    let fps_s = apply_fps.clone();
    let mute_s = apply_mute.clone();
    let vol_s = apply_vol.clone();
    let fit_s = apply_fit.clone();
    let fps_row_s = fps_row.clone();
    let mute_row_s = mute_row.clone();
    let vol_row_s = vol_row.clone();
    let suppress_apply_fps = Rc::new(Cell::new(false));
    let suppress_s = Rc::clone(&suppress_apply_fps);
    flow.connect_selected_children_changed(move |flow| {
        let Some(child) = flow.selected_children().into_iter().next() else {
            return;
        };
        let Some(path) = child_path(&child) else {
            return;
        };
        *selected_s.lock().unwrap() = Some(path.clone());
        let video = is_video(&path);
        set_video_playback_rows_visible(&fps_row_s, &mute_row_s, &vol_row_s, video);
        refresh_bg_music_ui(
            Some(&path),
            &music_btn_s,
            &music_archive_s,
            &music_yt_s,
            &remove_music_s,
            &bg_mute_s,
            &bg_vol_s,
            &bg_mute_row_s,
            &bg_vol_row_s,
            &music_name_s,
            &suppress_bg_s,
        );
        let c = Config::load(&default_config_path()).unwrap_or_default();
        suppress_s.set(true);
        fps_s.set_value(c.fps as f64);
        mute_s.set_active(c.mute);
        vol_s.set_value((c.volume * 100.0) as f64);
        fit_s.set_selected(match c.fit {
            FitMode::Cover => 0,
            FitMode::Contain => 1,
            FitMode::Stretch => 2,
        });
        suppress_s.set(false);
        show_preview(
            &path,
            &preview_pic_s,
            &preview_name_s,
            &preview_meta_s,
            &apply_s,
            &delete_s,
            &live_s,
            &gen_s,
            video && c.fps > 0,
            &preview_loading_s,
            Some(&child),
        );
        let my_stats = stats_gen_s.fetch_add(1, Ordering::Relaxed) + 1;
        schedule_local_stats(path, None, &preview_stats_s.refs(), &stats_gen_s, my_stats);
    });

    let selected_f = Arc::clone(&selected);
    let preview_pic_f = preview_pic.clone();
    let preview_name_f = preview_name.clone();
    let preview_meta_f = preview_meta.clone();
    let preview_loading_f = preview_loading.clone();
    let apply_f = apply_btn.clone();
    let delete_f = delete_btn.clone();
    let live_f = Arc::clone(&live_preview);
    let gen_f = Arc::clone(&preview_gen);
    let suppress_f = Rc::clone(&suppress_apply_fps);
    let prev_fps = Rc::new(Cell::new(-1.0f64));
    apply_fps.connect_value_changed(move |s| {
        if suppress_f.get() {
            prev_fps.set(s.value());
            return;
        }
        let v = s.value();
        let prev = prev_fps.replace(v);
        if prev >= 0.0 && (prev <= 0.0) == (v <= 0.0) {
            return;
        }
        let Some(path) = selected_f.lock().unwrap().clone() else {
            return;
        };
        if !is_video(&path) {
            return;
        }
        show_preview(
            &path,
            &preview_pic_f,
            &preview_name_f,
            &preview_meta_f,
            &apply_f,
            &delete_f,
            &live_f,
            &gen_f,
            v > 0.0,
            &preview_loading_f,
            None,
        );
    });

    let selected_a = Arc::clone(&selected);
    let monitors_a = Rc::clone(&selected_monitors);
    let status_a = status.clone();
    let fps_a = apply_fps.clone();
    let mute_a = apply_mute.clone();
    let vol_a = apply_vol.clone();
    let fit_a = apply_fit.clone();
    let sync_a = Arc::clone(&sync_mocks);
    let live_a = Arc::clone(&live_preview);
    apply_btn.connect_clicked(move |_| {
        let Some(path) = selected_a.lock().unwrap().clone() else {
            status_a.set_text("Select a wallpaper first");
            return;
        };
        let outs = monitors_a.borrow().clone();
        if outs.is_empty() {
            status_a.set_text("Select at least one monitor");
            return;
        }
        stop_live_preview(&live_a);
        match apply_wallpaper(&path, &outs) {
            Ok(()) => {
                let play = push_playback(&fps_a, &mute_a, &vol_a, &fit_a, is_video(&path));
                let msg = format!("Applied to {}: {}", outs.join(", "), path.display());
                status_a.set_text(&match play {
                    Ok(()) => msg,
                    Err(e) => format!("{msg} (playback: {e:#})"),
                });
                sync_a();
            }
            Err(e) => status_a.set_text(&format!("Failed: {e:#}")),
        }
    });

    let win_d = window.clone();
    let selected_d = Arc::clone(&selected);
    let status_d = status.clone();
    let flow_d = flow.clone();
    let body_d = gallery_body.clone();
    let filter_d = filter.clone();
    let search_d = library_search.clone();
    let dirs_d = dirs.clone();
    let preview_pic_d = preview_pic.clone();
    let preview_name_d = preview_name.clone();
    let preview_meta_d = preview_meta.clone();
    let preview_stats_d = preview_stats.clone();
    let apply_d = apply_btn.clone();
    let delete_d = delete_btn.clone();
    let music_btn_d = music_file_btn.clone();
    let music_archive_d = music_archive_btn.clone();
    let music_yt_d = music_yt_btn.clone();
    let remove_music_d = remove_music_btn.clone();
    let music_name_d = music_name.clone();
    let bg_mute_d = bg_mute.clone();
    let bg_vol_d = bg_vol.clone();
    let bg_mute_row_d = bg_mute_row.clone();
    let bg_vol_row_d = bg_vol_row.clone();
    let suppress_bg_d = Rc::clone(&suppress_bg_music);
    let live_d = Arc::clone(&live_preview);
    let fps_row_d = fps_row.clone();
    let mute_row_d = mute_row.clone();
    let vol_row_d = vol_row.clone();
    let sync_d = Arc::clone(&sync_mocks);
    delete_btn.connect_clicked(move |_| {
        let Some(path) = selected_d.lock().unwrap().clone() else {
            status_d.set_text("Select a wallpaper first");
            return;
        };
        if !path.is_file() {
            status_d.set_text("Nothing to delete — select a local file");
            return;
        }
        let fname = catalog::library_caption(&path);
        let alert = adw::AlertDialog::new(
            Some(&format!("Delete {fname}?")),
            Some("This permanently removes the file from disk."),
        );
        alert.add_response("cancel", "Cancel");
        alert.add_response("delete", "Delete");
        alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        let selected = Arc::clone(&selected_d);
        let status = status_d.clone();
        let body = body_d.clone();
        let flow = flow_d.clone();
        let filter = filter_d.clone();
        let search = search_d.clone();
        let dirs = dirs_d.clone();
        let pic = preview_pic_d.clone();
        let name = preview_name_d.clone();
        let meta = preview_meta_d.clone();
        let stats = preview_stats_d.clone();
        let apply = apply_d.clone();
        let delete = delete_d.clone();
        let music_btn = music_btn_d.clone();
        let music_archive = music_archive_d.clone();
        let music_yt = music_yt_d.clone();
        let remove_music = remove_music_d.clone();
        let music_name = music_name_d.clone();
        let bg_mute = bg_mute_d.clone();
        let bg_vol = bg_vol_d.clone();
        let bg_mute_row = bg_mute_row_d.clone();
        let bg_vol_row = bg_vol_row_d.clone();
        let suppress_bg = Rc::clone(&suppress_bg_d);
        let live = Arc::clone(&live_d);
        let fps_row = fps_row_d.clone();
        let mute_row = mute_row_d.clone();
        let vol_row = vol_row_d.clone();
        let sync = Arc::clone(&sync_d);
        alert.connect_response(None, move |_, response| {
            if response != "delete" {
                return;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    stop_live_preview(&live);
                    *selected.lock().unwrap() = None;
                    pic.set_paintable(Option::<&gdk::Paintable>::None);
                    set_preview_title(&name, "Click a wallpaper");
                    meta.set_text("Select monitors above, then Apply.");
                    stats.clear();
                    apply.set_sensitive(false);
                    delete.set_sensitive(false);
                    set_video_playback_rows_visible(&fps_row, &mute_row, &vol_row, false);
                    refresh_bg_music_ui(
                        None,
                        &music_btn,
                        &music_archive,
                        &music_yt,
                        &remove_music,
                        &bg_mute,
                        &bg_vol,
                        &bg_mute_row,
                        &bg_vol_row,
                        &music_name,
                        &suppress_bg,
                    );
                    flow.unselect_all();
                    refresh_gallery(&body, &flow, &dirs, filter.selected(), &search.text());
                    status.set_text(&format!("Deleted {fname}"));
                    catalog::remove_path_meta(&path);
                    sync();
                }
                Err(e) => status.set_text(&format!("Delete failed: {e}")),
            }
        });
        alert.present(Some(&win_d));
    });

    let win_m = window.clone();
    let selected_m = Arc::clone(&selected);
    let status_m = status.clone();
    let music_btn_m = music_file_btn.clone();
    let music_archive_m = music_archive_btn.clone();
    let music_yt_m = music_yt_btn.clone();
    let remove_music_m = remove_music_btn.clone();
    let music_name_m = music_name.clone();
    let bg_mute_m = bg_mute.clone();
    let bg_vol_m = bg_vol.clone();
    let bg_mute_row_m = bg_mute_row.clone();
    let bg_vol_row_m = bg_vol_row.clone();
    let suppress_bg_m = Rc::clone(&suppress_bg_music);
    music_file_btn.connect_clicked(move |_| {
        let Some(wall) = selected_m.lock().unwrap().clone() else {
            status_m.set_text("Select a wallpaper first");
            return;
        };
        if !wall.is_file() {
            status_m.set_text("Background music needs a local wallpaper file");
            return;
        }
        let dialog = FileDialog::builder().title("Add background music").build();
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        let audio = gtk::FileFilter::new();
        audio.set_name(Some("Audio"));
        for mime in [
            "audio/mpeg",
            "audio/ogg",
            "audio/flac",
            "audio/mp4",
            "audio/x-m4a",
            "audio/wav",
            "audio/x-wav",
            "audio/aac",
            "audio/opus",
        ] {
            audio.add_mime_type(mime);
        }
        for pat in [
            "*.mp3", "*.ogg", "*.oga", "*.flac", "*.m4a", "*.aac", "*.wav", "*.opus", "*.wma",
        ] {
            audio.add_pattern(pat);
        }
        filters.append(&audio);
        let any = gtk::FileFilter::new();
        any.set_name(Some("All files"));
        any.add_pattern("*");
        filters.append(&any);
        dialog.set_filters(Some(&filters));
        dialog.set_default_filter(Some(&audio));
        let status = status_m.clone();
        let music_btn = music_btn_m.clone();
        let music_archive = music_archive_m.clone();
        let music_yt = music_yt_m.clone();
        let remove_music = remove_music_m.clone();
        let music_name = music_name_m.clone();
        let bg_mute = bg_mute_m.clone();
        let bg_vol = bg_vol_m.clone();
        let bg_mute_row = bg_mute_row_m.clone();
        let bg_vol_row = bg_vol_row_m.clone();
        let suppress_bg = Rc::clone(&suppress_bg_m);
        dialog.open(
            Some(&win_m),
            Option::<&gio::Cancellable>::None,
            move |res| match res {
                Ok(file) => {
                    let Some(path) = file.path() else {
                        status.set_text("Could not read audio path");
                        return;
                    };
                    if !is_audio(&path) {
                        status.set_text("Pick an audio file (mp3, ogg, flac, m4a, wav, …)");
                        return;
                    }
                    let volume = (bg_vol.value() as f32) / 100.0;
                    match client_request(&Request::SetBgMusic {
                        wallpaper: wall.clone(),
                        music: Some(path.clone()),
                    })
                    .and_then(ipc_ok)
                    {
                        Ok(()) => {
                            let _ = client_request(&Request::SetBgMusicVolume {
                                wallpaper: wall.clone(),
                                volume,
                            });
                            refresh_bg_music_ui(
                                Some(&wall),
                                &music_btn,
                                &music_archive,
                                &music_yt,
                                &remove_music,
                                &bg_mute,
                                &bg_vol,
                                &bg_mute_row,
                                &bg_vol_row,
                                &music_name,
                                &suppress_bg,
                            );
                            status.set_text(&format!(
                                "Background music: {}",
                                path.file_name().and_then(|s| s.to_str()).unwrap_or("audio")
                            ));
                        }
                        Err(e) => status.set_text(&format!("Music failed: {e:#}")),
                    }
                }
                Err(e) if e.matches(gtk::DialogError::Dismissed) => {}
                Err(e) => status.set_text(&format!("Music picker: {e}")),
            },
        );
    });

    {
        let win = window.clone();
        let selected = Arc::clone(&selected);
        let status = status.clone();
        let music_btn = music_file_btn.clone();
        let music_archive = music_archive_btn.clone();
        let music_yt = music_yt_btn.clone();
        let remove_music = remove_music_btn.clone();
        let music_name = music_name.clone();
        let bg_mute = bg_mute.clone();
        let bg_vol = bg_vol.clone();
        let bg_mute_row = bg_mute_row.clone();
        let bg_vol_row = bg_vol_row.clone();
        music_archive_btn.connect_clicked(move |_| {
            let Some(wall) = selected.lock().unwrap().clone() else {
                status.set_text("Select a wallpaper first");
                return;
            };
            if !wall.is_file() {
                status.set_text("Background music needs a local wallpaper file");
                return;
            }
            open_archive_music_dialog(
                &win,
                wall,
                &status,
                &music_btn,
                &music_archive,
                &music_yt,
                &remove_music,
                &bg_mute,
                &bg_vol,
                &bg_mute_row,
                &bg_vol_row,
                &music_name,
            );
        });
    }

    {
        let win = window.clone();
        let selected = Arc::clone(&selected);
        let status = status.clone();
        let music_btn = music_file_btn.clone();
        let music_archive = music_archive_btn.clone();
        let music_yt = music_yt_btn.clone();
        let remove_music = remove_music_btn.clone();
        let music_name = music_name.clone();
        let bg_mute = bg_mute.clone();
        let bg_vol = bg_vol.clone();
        let bg_mute_row = bg_mute_row.clone();
        let bg_vol_row = bg_vol_row.clone();
        music_yt_btn.connect_clicked(move |_| {
            let Some(wall) = selected.lock().unwrap().clone() else {
                status.set_text("Select a wallpaper first");
                return;
            };
            if !wall.is_file() {
                status.set_text("Background music needs a local wallpaper file");
                return;
            }
            open_yt_music_dialog(
                &win,
                wall,
                &status,
                &music_btn,
                &music_archive,
                &music_yt,
                &remove_music,
                &bg_mute,
                &bg_vol,
                &bg_mute_row,
                &bg_vol_row,
                &music_name,
            );
        });
    }

    let selected_rm = Arc::clone(&selected);
    let status_rm = status.clone();
    let music_btn_rm = music_file_btn.clone();
    let music_archive_rm = music_archive_btn.clone();
    let music_yt_rm = music_yt_btn.clone();
    let remove_music_rm = remove_music_btn.clone();
    let music_name_rm = music_name.clone();
    let bg_mute_rm = bg_mute.clone();
    let bg_vol_rm = bg_vol.clone();
    let bg_mute_row_rm = bg_mute_row.clone();
    let bg_vol_row_rm = bg_vol_row.clone();
    let suppress_bg_rm = Rc::clone(&suppress_bg_music);
    remove_music_btn.connect_clicked(move |_| {
        let Some(wall) = selected_rm.lock().unwrap().clone() else {
            status_rm.set_text("Select a wallpaper first");
            return;
        };
        match client_request(&Request::SetBgMusic {
            wallpaper: wall.clone(),
            music: None,
        })
        .and_then(ipc_ok)
        {
            Ok(()) => {
                refresh_bg_music_ui(
                    Some(&wall),
                    &music_btn_rm,
                    &music_archive_rm,
                    &music_yt_rm,
                    &remove_music_rm,
                    &bg_mute_rm,
                    &bg_vol_rm,
                    &bg_mute_row_rm,
                    &bg_vol_row_rm,
                    &music_name_rm,
                    &suppress_bg_rm,
                );
                status_rm.set_text("Background music removed");
            }
            Err(e) => status_rm.set_text(&format!("Remove music failed: {e:#}")),
        }
    });

    {
        let file_btn = music_file_btn.clone();
        let archive_btn = music_archive_btn.clone();
        let yt_btn = music_yt_btn.clone();
        let remove_btn = remove_music_btn.clone();
        let mute = bg_mute.clone();
        let vol = bg_vol.clone();
        let mute_row = bg_mute_row.clone();
        let vol_row = bg_vol_row.clone();
        let name_lbl = music_name.clone();
        let suppress = Rc::clone(&suppress_bg_music);
        let selected_w = Arc::clone(&selected);
        watch_sources(&source_watchers, move || {
            let wall = selected_w.lock().unwrap().clone();
            refresh_bg_music_ui(
                wall.as_deref(),
                &file_btn,
                &archive_btn,
                &yt_btn,
                &remove_btn,
                &mute,
                &vol,
                &mute_row,
                &vol_row,
                &name_lbl,
                &suppress,
            );
        });
        refresh_bg_music_ui(
            None,
            &music_file_btn,
            &music_archive_btn,
            &music_yt_btn,
            &remove_music_btn,
            &bg_mute,
            &bg_vol,
            &bg_mute_row,
            &bg_vol_row,
            &music_name,
            &suppress_bg_music,
        );
    }

    let selected_bm = Arc::clone(&selected);
    let suppress_bm = Rc::clone(&suppress_bg_music);
    let status_bm = status.clone();
    bg_mute.connect_active_notify(move |sw| {
        if suppress_bm.get() {
            return;
        }
        let Some(wall) = selected_bm.lock().unwrap().clone() else {
            return;
        };
        match client_request(&Request::SetBgMusicMute {
            wallpaper: Some(wall),
            mute: sw.is_active(),
        })
        .and_then(ipc_ok)
        {
            Ok(()) => {}
            Err(e) => status_bm.set_text(&format!("Music mute: {e:#}")),
        }
    });

    let selected_bv = Arc::clone(&selected);
    let suppress_bv = Rc::clone(&suppress_bg_music);
    let bg_mute_bv = bg_mute.clone();
    let status_bv = status.clone();
    bg_vol.connect_value_changed(move |spin| {
        if suppress_bv.get() {
            return;
        }
        let Some(wall) = selected_bv.lock().unwrap().clone() else {
            return;
        };
        let volume = (spin.value() as f32) / 100.0;
        match client_request(&Request::SetBgMusicVolume {
            wallpaper: wall,
            volume,
        })
        .and_then(ipc_ok)
        {
            Ok(()) => {
                if volume > 0.0 && bg_mute_bv.is_active() {
                    suppress_bv.set(true);
                    bg_mute_bv.set_active(false);
                    suppress_bv.set(false);
                }
            }
            Err(e) => status_bv.set_text(&format!("Music volume: {e:#}")),
        }
    });

    let win = window.clone();
    let body_o = gallery_body.clone();
    let flow_o = flow.clone();
    let filter_o = filter.clone();
    let search_o = library_search.clone();
    let status_o = status.clone();
    let selected_o = Arc::clone(&selected);
    let preview_pic_o = preview_pic.clone();
    let preview_name_o = preview_name.clone();
    let preview_meta_o = preview_meta.clone();
    let preview_loading_o = preview_loading.clone();
    let preview_stats_o = preview_stats.clone();
    let apply_o = apply_btn.clone();
    let delete_o = delete_btn.clone();
    let music_btn_o = music_file_btn.clone();
    let music_archive_o = music_archive_btn.clone();
    let music_yt_o = music_yt_btn.clone();
    let remove_music_o = remove_music_btn.clone();
    let music_name_o = music_name.clone();
    let bg_mute_o = bg_mute.clone();
    let bg_vol_o = bg_vol.clone();
    let bg_mute_row_o = bg_mute_row.clone();
    let bg_vol_row_o = bg_vol_row.clone();
    let suppress_bg_o = Rc::clone(&suppress_bg_music);
    let live_o = Arc::clone(&live_preview);
    let gen_o = Arc::clone(&preview_gen);
    let fps_o = apply_fps.clone();
    let fps_row_o = fps_row.clone();
    let mute_row_o = mute_row.clone();
    let vol_row_o = vol_row.clone();
    add_wallpaper_btn.connect_clicked(move |_| {
        let dialog = FileDialog::builder().title("Add wallpaper").build();
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        let any = gtk::FileFilter::new();
        any.set_name(Some("All files"));
        any.add_pattern("*");
        filters.append(&any);
        dialog.set_filters(Some(&filters));
        dialog.set_default_filter(Some(&any));
        if let Some(home) = glib::home_dir().to_str() {
            let downloads = gio::File::for_path(format!("{home}/Downloads"));
            if downloads.query_exists(gio::Cancellable::NONE) {
                dialog.set_initial_folder(Some(&downloads));
            }
        }
        let parent = win.clone();
        let body = body_o.clone();
        let flow = flow_o.clone();
        let filter = filter_o.clone();
        let search = search_o.clone();
        let status = status_o.clone();
        let selected = Arc::clone(&selected_o);
        let preview_pic = preview_pic_o.clone();
        let preview_name = preview_name_o.clone();
        let preview_meta = preview_meta_o.clone();
        let preview_loading = preview_loading_o.clone();
        let preview_stats = preview_stats_o.clone();
        let apply = apply_o.clone();
        let delete = delete_o.clone();
        let music_btn = music_btn_o.clone();
        let music_archive = music_archive_o.clone();
        let music_yt = music_yt_o.clone();
        let remove_music = remove_music_o.clone();
        let music_name = music_name_o.clone();
        let bg_mute = bg_mute_o.clone();
        let bg_vol = bg_vol_o.clone();
        let bg_mute_row = bg_mute_row_o.clone();
        let bg_vol_row = bg_vol_row_o.clone();
        let suppress_bg = Rc::clone(&suppress_bg_o);
        let live = Arc::clone(&live_o);
        let gen = Arc::clone(&gen_o);
        let fps = fps_o.clone();
        let fps_row = fps_row_o.clone();
        let mute_row = mute_row_o.clone();
        let vol_row = vol_row_o.clone();
        dialog.open(
            Some(&win),
            Option::<&gio::Cancellable>::None,
            move |res| match res {
                Ok(file) => {
                    if let Some(path) = file.path() {
                        after_wallpaper_added(
                            path,
                            None,
                            &body,
                            &flow,
                            &filter,
                            &search,
                            &status,
                            &selected,
                            &preview_pic,
                            &preview_name,
                            &preview_meta,
                            &preview_loading,
                            &preview_stats,
                            &apply,
                            &delete,
                            &music_btn,
                            &music_archive,
                            &music_yt,
                            &remove_music,
                            &music_name,
                            &bg_mute,
                            &bg_vol,
                            &bg_mute_row,
                            &bg_vol_row,
                            &suppress_bg,
                            &live,
                            &gen,
                            &fps,
                            &fps_row,
                            &mute_row,
                            &vol_row,
                        );
                    } else {
                        let alert = adw::AlertDialog::new(
                            Some("Could not read file path"),
                            Some("Try copying the file into ~/Pictures/Wallpapers."),
                        );
                        alert.add_response("ok", "OK");
                        alert.present(Some(&parent));
                    }
                }
                Err(e) if e.matches(gtk::DialogError::Dismissed) => {}
                Err(e) => {
                    let alert = adw::AlertDialog::new(Some("Add failed"), Some(&e.to_string()));
                    alert.add_response("ok", "OK");
                    alert.present(Some(&parent));
                }
            },
        );
    });

    let win_yt = window.clone();
    let add_wallpaper_btn_yt = add_wallpaper_btn.clone();
    let add_more_btn_yt = add_more_btn.clone();
    let add_more_popover_yt = add_more_popover.clone();
    let body_yt = gallery_body.clone();
    let flow_yt = flow.clone();
    let filter_yt = filter.clone();
    let search_yt = library_search.clone();
    let status_yt = status.clone();
    let selected_yt = Arc::clone(&selected);
    let preview_pic_yt = preview_pic.clone();
    let preview_name_yt = preview_name.clone();
    let preview_meta_yt = preview_meta.clone();
    let preview_loading_yt = preview_loading.clone();
    let preview_stats_yt = preview_stats.clone();
    let apply_yt = apply_btn.clone();
    let delete_yt = delete_btn.clone();
    let music_btn_yt = music_file_btn.clone();
    let music_archive_yt = music_archive_btn.clone();
    let music_ytmusic_yt = music_yt_btn.clone();
    let remove_music_yt = remove_music_btn.clone();
    let music_name_yt = music_name.clone();
    let bg_mute_yt = bg_mute.clone();
    let bg_vol_yt = bg_vol.clone();
    let bg_mute_row_yt = bg_mute_row.clone();
    let bg_vol_row_yt = bg_vol_row.clone();
    let suppress_bg_yt = Rc::clone(&suppress_bg_music);
    let live_yt = Arc::clone(&live_preview);
    let gen_yt = Arc::clone(&preview_gen);
    let fps_yt = apply_fps.clone();
    let fps_row_yt = fps_row.clone();
    let mute_row_yt = mute_row.clone();
    let vol_row_yt = vol_row.clone();
    add_youtube_btn.connect_clicked(move |_| {
        add_more_popover_yt.popdown();
        let url_entry = Entry::new();
        url_entry.set_placeholder_text(Some("https://youtu.be/…"));
        url_entry.set_hexpand(true);
        url_entry.set_width_chars(42);
        let alert = adw::AlertDialog::new(
            Some("Add from YouTube"),
            Some(
                "Paste a video URL. It will be downloaded into your wallpaper library with yt-dlp.",
            ),
        );
        alert.set_extra_child(Some(&url_entry));
        alert.add_response("cancel", "Cancel");
        alert.add_response("download", "Download");
        alert.set_response_appearance("download", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("download"));
        alert.set_close_response("cancel");

        let parent = win_yt.clone();
        let add_btn = add_wallpaper_btn_yt.clone();
        let more_btn = add_more_btn_yt.clone();
        let body = body_yt.clone();
        let flow = flow_yt.clone();
        let filter = filter_yt.clone();
        let search = search_yt.clone();
        let status = status_yt.clone();
        let selected = Arc::clone(&selected_yt);
        let preview_pic = preview_pic_yt.clone();
        let preview_name = preview_name_yt.clone();
        let preview_meta = preview_meta_yt.clone();
        let preview_loading = preview_loading_yt.clone();
        let preview_stats = preview_stats_yt.clone();
        let apply = apply_yt.clone();
        let delete = delete_yt.clone();
        let music_btn = music_btn_yt.clone();
        let music_archive = music_archive_yt.clone();
        let music_yt = music_ytmusic_yt.clone();
        let remove_music = remove_music_yt.clone();
        let music_name = music_name_yt.clone();
        let bg_mute = bg_mute_yt.clone();
        let bg_vol = bg_vol_yt.clone();
        let bg_mute_row = bg_mute_row_yt.clone();
        let bg_vol_row = bg_vol_row_yt.clone();
        let suppress_bg = Rc::clone(&suppress_bg_yt);
        let live = Arc::clone(&live_yt);
        let gen = Arc::clone(&gen_yt);
        let fps = fps_yt.clone();
        let fps_row = fps_row_yt.clone();
        let mute_row = mute_row_yt.clone();
        let vol_row = vol_row_yt.clone();
        alert.connect_response(None, move |_, response| {
            if response != "download" {
                return;
            }
            let url = url_entry.text().trim().to_string();
            if let Err(msg) = validate_youtube_url(&url) {
                let err = adw::AlertDialog::new(Some("Invalid URL"), Some(&msg));
                err.add_response("ok", "OK");
                err.present(Some(&parent));
                return;
            }
            if !yt_dlp_available() {
                let err = adw::AlertDialog::new(
                    Some("yt-dlp not found"),
                    Some("Install yt-dlp and ensure it is on your PATH, then try again."),
                );
                err.add_response("ok", "OK");
                err.present(Some(&parent));
                return;
            }

            let library = Config::load(&default_config_path())
                .unwrap_or_default()
                .library;
            add_btn.set_sensitive(false);
            more_btn.set_sensitive(false);
            status.set_text("Downloading from YouTube…");

            let (tx, rx) = std::sync::mpsc::channel::<Result<PathBuf, String>>();
            std::thread::spawn(move || {
                let _ = tx.send(download_youtube_to_library(&url, &library));
            });

            let parent = parent.clone();
            let add_btn = add_btn.clone();
            let more_btn = more_btn.clone();
            let body = body.clone();
            let flow = flow.clone();
            let filter = filter.clone();
            let search = search.clone();
            let status = status.clone();
            let selected = Arc::clone(&selected);
            let preview_pic = preview_pic.clone();
            let preview_name = preview_name.clone();
            let preview_meta = preview_meta.clone();
            let preview_loading = preview_loading.clone();
            let preview_stats = preview_stats.clone();
            let apply = apply.clone();
            let delete = delete.clone();
            let music_btn = music_btn.clone();
            let music_archive = music_archive.clone();
            let music_yt = music_yt.clone();
            let remove_music = remove_music.clone();
            let music_name = music_name.clone();
            let bg_mute = bg_mute.clone();
            let bg_vol = bg_vol.clone();
            let bg_mute_row = bg_mute_row.clone();
            let bg_vol_row = bg_vol_row.clone();
            let suppress_bg = Rc::clone(&suppress_bg);
            let live = Arc::clone(&live);
            let gen = Arc::clone(&gen);
            let fps = fps.clone();
            let fps_row = fps_row.clone();
            let mute_row = mute_row.clone();
            let vol_row = vol_row.clone();
            glib::timeout_add_local(Duration::from_millis(100), move || match rx.try_recv() {
                Ok(result) => {
                    add_btn.set_sensitive(true);
                    more_btn.set_sensitive(true);
                    match result {
                        Ok(path) => {
                            let caption = catalog::library_caption(&path);
                            after_wallpaper_added(
                                path,
                                Some(&format!("Downloaded {caption}")),
                                &body,
                                &flow,
                                &filter,
                                &search,
                                &status,
                                &selected,
                                &preview_pic,
                                &preview_name,
                                &preview_meta,
                                &preview_loading,
                                &preview_stats,
                                &apply,
                                &delete,
                                &music_btn,
                                &music_archive,
                                &music_yt,
                                &remove_music,
                                &music_name,
                                &bg_mute,
                                &bg_vol,
                                &bg_mute_row,
                                &bg_vol_row,
                                &suppress_bg,
                                &live,
                                &gen,
                                &fps,
                                &fps_row,
                                &mute_row,
                                &vol_row,
                            );
                        }
                        Err(e) => {
                            status.set_text(&format!("YouTube download failed: {e}"));
                            let err =
                                adw::AlertDialog::new(Some("YouTube download failed"), Some(&e));
                            err.add_response("ok", "OK");
                            err.present(Some(&parent));
                        }
                    }
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    add_btn.set_sensitive(true);
                    more_btn.set_sensitive(true);
                    status.set_text("YouTube download failed: worker exited");
                    glib::ControlFlow::Break
                }
            });
        });
        alert.present(Some(&win_yt));
    });

    settings.wire_persistence();
    let mocks_tick = Arc::clone(&monitor_mocks);
    let status_tick = status.clone();
    glib::timeout_add_local(Duration::from_secs(2), move || {
        if refresh_monitor_mocks(&mocks_tick) {
            refresh_status(&status_tick);
        }
        glib::ControlFlow::Continue
    });
    bind_preview_visibility_pause(&window, &live_preview);
    let live_close = Arc::clone(&live_preview);
    window.connect_close_request(move |_| {
        stop_live_preview(&live_close);
        glib::Propagation::Proceed
    });
    let live_sd = Arc::clone(&live_preview);
    app.connect_shutdown(move |_| {
        stop_live_preview(&live_sd);
    });
    window.present();
}

fn after_wallpaper_added(
    path: PathBuf,
    status_msg: Option<&str>,
    body: &Stack,
    flow: &FlowBox,
    filter: &DropDown,
    search: &Entry,
    status: &Label,
    selected: &Mutex<Option<PathBuf>>,
    preview_pic: &Picture,
    preview_name: &Label,
    preview_meta: &Label,
    preview_loading: &PreviewLoading,
    preview_stats: &StatsPane,
    apply: &Button,
    delete: &Button,
    music_btn: &Button,
    music_archive: &Button,
    music_yt: &Button,
    remove_music: &Button,
    music_name: &Label,
    bg_mute: &Switch,
    bg_vol: &SpinButton,
    bg_mute_row: &GtkBox,
    bg_vol_row: &GtkBox,
    suppress_bg: &Rc<Cell<bool>>,
    live: &Arc<Mutex<Option<PreviewSession>>>,
    gen: &Arc<AtomicU64>,
    fps: &SpinButton,
    fps_row: &GtkBox,
    mute_row: &GtkBox,
    vol_row: &GtkBox,
) {
    let video = is_video(&path);
    set_video_playback_rows_visible(fps_row, mute_row, vol_row, video);
    show_preview(
        &path,
        preview_pic,
        preview_name,
        preview_meta,
        apply,
        delete,
        live,
        gen,
        video && fps.value() > 0.0,
        preview_loading,
        None,
    );
    refresh_bg_music_ui(
        Some(&path),
        music_btn,
        music_archive,
        music_yt,
        remove_music,
        bg_mute,
        bg_vol,
        bg_mute_row,
        bg_vol_row,
        music_name,
        suppress_bg,
    );
    schedule_local_stats(
        path.clone(),
        None,
        &preview_stats.refs(),
        &Arc::new(AtomicU64::new(0)),
        1,
    );
    *selected.lock().unwrap() = Some(path.clone());
    let fallback = format!("Selected {}", path.display());
    status.set_text(status_msg.unwrap_or(&fallback));
    let dirs = gallery_dirs(
        &Config::load(&default_config_path())
            .unwrap_or_default()
            .library,
    );
    refresh_gallery(body, flow, &dirs, filter.selected(), &search.text());
}

fn validate_youtube_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("Enter a YouTube URL.".into());
    }
    let lower = url.to_ascii_lowercase();
    let looks_yt = lower.contains("youtube.com/")
        || lower.contains("youtu.be/")
        || lower.contains("youtube-nocookie.com/");
    let looks_url = lower.starts_with("http://") || lower.starts_with("https://");
    if !looks_yt && !looks_url {
        return Err("Enter a youtube.com or youtu.be URL.".into());
    }
    Ok(())
}

fn expand_library_path(raw: &str) -> PathBuf {
    let raw = raw.trim();
    if raw.is_empty() {
        return PathBuf::new();
    }
    if raw == "~" {
        return glib::home_dir();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return glib::home_dir().join(rest);
    }
    PathBuf::from(raw)
}

fn yt_dlp_available() -> bool {
    match Command::new("yt-dlp")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(_) => false,
    }
}

fn download_youtube_to_library(url: &str, library: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(library).map_err(|e| format!("Could not create library dir: {e}"))?;
    let out_tmpl = library.join("%(title).200B [%(id)s].%(ext)s");
    let out_tmpl = out_tmpl
        .to_str()
        .ok_or_else(|| "Library path is not valid UTF-8".to_string())?;

    let output = Command::new("yt-dlp")
        .args([
            "-f",
            "bv*+ba/b",
            "--merge-output-format",
            "mp4",
            "--no-playlist",
            "--no-progress",
            "--print",
            "after_move:filepath",
            "-o",
            out_tmpl,
            url,
        ])
        .output()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                "yt-dlp not found. Install yt-dlp and ensure it is on your PATH.".into()
            } else {
                format!("Could not run yt-dlp: {e}")
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr.trim();
        if msg.is_empty() {
            return Err(format!("yt-dlp exited with {}", output.status));
        }
        let last = msg
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .last()
            .unwrap_or(msg);
        return Err(last.to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let path_str = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .last()
        .unwrap_or("");
    if path_str.is_empty() {
        return Err("yt-dlp finished but did not report an output file".into());
    }
    let path = PathBuf::from(path_str);
    if !path.is_file() {
        return Err(format!("Downloaded file missing: {}", path.display()));
    }
    Ok(path)
}
