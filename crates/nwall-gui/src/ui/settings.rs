
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{
    Align, Box as GtkBox, Button, CheckButton, DropDown, Entry, Grid, Label,
    Orientation, PasswordEntry, PolicyType, ScrolledWindow, SpinButton, StringList, Switch,
};
use nwall_catalog as catalog;
use nwall_ipc::{client_request, default_config_path, is_library_source, normalize_preview_width_pct,
    resolve_catalog_source, Config, FitMode, PausePolicy, Request,
    Slideshow,
};

use crate::app::{
    fire_source_watchers, replace_string_list, visible_catalog_sources, watch_sources, SourceWatchers,
};
use crate::consts::*;
use crate::widgets::{
    filter_heading, labeled_row, section_label, spin_with_minutes, spin_with_percent,
};

pub(crate) struct SettingsUi {
    pub(crate) root: ScrolledWindow,
    pub(crate) fps: SpinButton,
    pub(crate) mute: Switch,
    pub(crate) volume: SpinButton,
    pub(crate) fit: DropDown,
    pub(crate) fs: CheckButton,
    pub(crate) overview: CheckButton,
    pub(crate) drag: CheckButton,
    pub(crate) covered: CheckButton,
    pub(crate) music_fs: CheckButton,
    pub(crate) music_overview: CheckButton,
    pub(crate) music_drag: CheckButton,
    pub(crate) music_covered: CheckButton,
    pub(crate) music_other_audio: CheckButton,
    pub(crate) theme: DropDown,
    pub(crate) library_entry: Entry,
    pub(crate) library_browse: Button,
    pub(crate) show_stats: Switch,
    pub(crate) fast_image_preview: Switch,
    pub(crate) fast_video_preview: Switch,
    pub(crate) preview_width: SpinButton,
    pub(crate) gui_zoom: SpinButton,
    pub(crate) ss_enabled: Switch,
    pub(crate) ss_show_tray: Switch,
    pub(crate) ss_interval: SpinButton,
    pub(crate) ss_source: DropDown,
    pub(crate) ss_tags: Entry,
    pub(crate) ss_source_names: Rc<RefCell<Vec<String>>>,
    pub(crate) ss_actual_source: Rc<RefCell<String>>,
    pub(crate) ss_suppress: Rc<Cell<bool>>,
}

impl SettingsUi {
    pub(crate) fn wire_persistence(&self) {
        let fps = self.fps.clone();
        fps.connect_value_changed(move |s| {
            let _ = client_request(&Request::SetFps {
                fps: s.value() as u32,
            });
            let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
            cfg.fps = s.value() as u32;
            let _ = cfg.save(&default_config_path());
        });

        let mute = self.mute.clone();
        mute.connect_active_notify(move |s| {
            let _ = client_request(&Request::SetMute {
                mute: s.is_active(),
            });
            let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
            cfg.mute = s.is_active();
            let _ = cfg.save(&default_config_path());
        });

        let volume = self.volume.clone();
        volume.connect_value_changed(move |s| {
            let v = (s.value() as f32) / 100.0;
            let _ = client_request(&Request::SetVolume { volume: v });
        });

        let fit = self.fit.clone();
        fit.connect_selected_notify(move |dd| {
            let mode = match dd.selected() {
                1 => FitMode::Contain,
                2 => FitMode::Stretch,
                _ => FitMode::Cover,
            };
            let _ = client_request(&Request::SetFit { fit: mode });
            let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
            cfg.fit = mode;
            let _ = cfg.save(&default_config_path());
        });

        let send_pause = {
            let fs = self.fs.clone();
            let overview = self.overview.clone();
            let drag = self.drag.clone();
            let covered = self.covered.clone();
            let music_fs = self.music_fs.clone();
            let music_overview = self.music_overview.clone();
            let music_drag = self.music_drag.clone();
            let music_covered = self.music_covered.clone();
            let music_other_audio = self.music_other_audio.clone();
            move || {
                let pause = PausePolicy {
                    on_fullscreen: fs.is_active(),
                    on_overview: overview.is_active(),
                    on_window_drag: drag.is_active(),
                    on_covered: covered.is_active(),
                    music_on_fullscreen: music_fs.is_active(),
                    music_on_overview: music_overview.is_active(),
                    music_on_window_drag: music_drag.is_active(),
                    music_on_covered: music_covered.is_active(),
                    music_on_other_audio: music_other_audio.is_active(),
                };
                let _ = client_request(&Request::SetPausePolicy {
                    pause: pause.clone(),
                });
                let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
                cfg.pause = pause;
                let _ = cfg.save(&default_config_path());
            }
        };

        let sp = send_pause.clone();
        self.fs.connect_toggled(move |_| sp());
        let sp = send_pause.clone();
        self.overview.connect_toggled(move |_| sp());
        let sp = send_pause.clone();
        self.drag.connect_toggled(move |_| sp());
        let sp = send_pause.clone();
        self.covered.connect_toggled(move |_| sp());
        let sp = send_pause.clone();
        self.music_fs.connect_toggled(move |_| sp());
        let sp = send_pause.clone();
        self.music_overview.connect_toggled(move |_| sp());
        let sp = send_pause.clone();
        self.music_drag.connect_toggled(move |_| sp());
        let sp = send_pause.clone();
        self.music_covered.connect_toggled(move |_| sp());
        let sp = send_pause;
        self.music_other_audio.connect_toggled(move |_| sp());

        let theme = self.theme.clone();
        theme.connect_selected_notify(move |dd| {
            let name = match dd.selected() {
                1 => "dark",
                2 => "light",
                _ => "system",
            };
            let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
            cfg.theme = name.into();
            let _ = cfg.save(&default_config_path());
            let mgr = adw::StyleManager::default();
            match name {
                "dark" => mgr.set_color_scheme(adw::ColorScheme::ForceDark),
                "light" => mgr.set_color_scheme(adw::ColorScheme::ForceLight),
                _ => mgr.set_color_scheme(adw::ColorScheme::Default),
            }
        });

        let send_ss = {
            let enabled = self.ss_enabled.clone();
            let show_tray = self.ss_show_tray.clone();
            let interval = self.ss_interval.clone();
            let tags = self.ss_tags.clone();
            let actual = Rc::clone(&self.ss_actual_source);
            move || persist_slideshow(&enabled, &show_tray, &interval, &actual.borrow(), &tags)
        };
        {
            let interval = self.ss_interval.clone();
            let show_tray = self.ss_show_tray.clone();
            let tags = self.ss_tags.clone();
            let actual = Rc::clone(&self.ss_actual_source);
            self.ss_enabled.connect_active_notify(move |sw| {
                persist_slideshow(sw, &show_tray, &interval, &actual.borrow(), &tags);
                if sw.is_active() {
                    let _ = client_request(&Request::SlideshowStart);
                } else {
                    let _ = client_request(&Request::SlideshowStop);
                }
            });
        }
        let ss = send_ss.clone();
        self.ss_show_tray.connect_active_notify(move |_| ss());
        let ss = send_ss.clone();
        self.ss_interval.connect_value_changed(move |_| ss());
        let ss = send_ss;
        self.ss_tags.connect_changed(move |_| ss());

        let suppress = Rc::clone(&self.ss_suppress);
        let names = Rc::clone(&self.ss_source_names);
        let actual = Rc::clone(&self.ss_actual_source);
        let enabled = self.ss_enabled.clone();
        let show_tray = self.ss_show_tray.clone();
        let interval = self.ss_interval.clone();
        let tags = self.ss_tags.clone();
        self.ss_source.connect_selected_notify(move |dd| {
            if suppress.get() {
                return;
            }
            let idx = dd.selected() as usize;
            let picked = names
                .borrow()
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "Wallhaven".into());
            let source_name = canonical_slideshow_source(&picked);
            *actual.borrow_mut() = source_name;
            persist_slideshow(&enabled, &show_tray, &interval, &actual.borrow(), &tags);
        });
    }
}

pub(crate) fn persist_slideshow(
    enabled: &Switch,
    show_tray: &Switch,
    interval: &SpinButton,
    source_name: &str,
    tags: &Entry,
) {
    let slideshow = Slideshow {
        enabled: enabled.is_active(),
        interval_minutes: (interval.value() as u32).clamp(1, 1440),
        source: canonical_slideshow_source(source_name),
        tags: tags.text().to_string(),
        show_in_tray: show_tray.is_active(),
    };
    let _ = client_request(&Request::SetSlideshow { slideshow });
}

pub(crate) fn canonical_slideshow_source(picked: &str) -> String {
    if is_library_source(picked) {
        return "library".into();
    }
    let picked = nwall_ipc::canonicalize_source_name(picked);
    let cfg = Config::load(&default_config_path()).unwrap_or_default();
    if let Some(s) = resolve_catalog_source(&cfg.sources, &picked) {
        return s.name.clone();
    }
    let t = picked.trim();
    if t.is_empty() {
        "Wallhaven".into()
    } else {
        t.to_string()
    }
}

pub(crate) fn slideshow_picker_label(source: &str) -> String {
    if is_library_source(source) {
        "Library (local files)".into()
    } else {
        canonical_slideshow_source(source)
    }
}

pub(crate) fn slideshow_picker_names() -> Vec<String> {
    let mut names: Vec<String> = visible_catalog_sources()
        .into_iter()
        .map(|s| s.name)
        .collect();
    names.push("Library (local files)".into());
    names
}

pub(crate) fn index_of_slideshow_source(names: &[String], source: &str) -> Option<usize> {
    let display = slideshow_picker_label(source);
    names
        .iter()
        .position(|n| n == &display)
        .or_else(|| names.iter().position(|n| n.eq_ignore_ascii_case(&display)))
        .or_else(|| {
            let lower = source.to_ascii_lowercase();
            if lower.contains("bing") {
                names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case("Bing Daily"))
            } else if lower.contains("wallhaven") {
                names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case("Wallhaven"))
            } else {
                None
            }
        })
}

pub(crate) fn rebuild_slideshow_dropdown(
    dd: &DropDown,
    list: &StringList,
    names_cell: &RefCell<Vec<String>>,
    actual: &RefCell<String>,
    suppress: &Cell<bool>,
) {
    let names = slideshow_picker_names();
    let current = actual.borrow().clone();
    suppress.set(true);
    replace_string_list(list, &names);
    *names_cell.borrow_mut() = names.clone();
    if let Some(i) = index_of_slideshow_source(&names, &current) {
        dd.set_selected(i as u32);
        if let Some(name) = names.get(i) {
            *actual.borrow_mut() = canonical_slideshow_source(name);
        }
    } else if !names.is_empty() {
        let i = names
            .iter()
            .position(|n| n.eq_ignore_ascii_case("Wallhaven"))
            .unwrap_or(0);
        dd.set_selected(i as u32);
    }
    suppress.set(false);
}

pub(crate) fn build_settings(cfg: &mut Config, source_watchers: SourceWatchers) -> SettingsUi {
    let page = GtkBox::new(Orientation::Vertical, 16);
    page.set_margin_top(20);
    page.set_margin_bottom(20);
    page.set_margin_start(24);
    page.set_margin_end(24);

    page.append(&section_label("Defaults"));

    page.append(&filter_heading("Playback"));
    let fps = SpinButton::with_range(0.0, 240.0, 1.0);
    fps.set_value(cfg.fps as f64);
    page.append(&labeled_row("FPS limit", &fps));

    let mute = Switch::new();
    mute.set_active(cfg.mute);
    mute.set_halign(Align::End);
    page.append(&labeled_row("Mute", &mute));

    let volume = SpinButton::with_range(0.0, 100.0, 1.0);
    volume.set_digits(0);
    volume.set_value((cfg.volume * 100.0) as f64);
    page.append(&labeled_row("Volume", spin_with_percent(&volume)));

    let fit_list = StringList::new(&["Cover", "Contain", "Stretch"]);
    let fit = DropDown::new(Some(fit_list), Option::<&gtk::Expression>::None);
    fit.set_selected(match cfg.fit {
        FitMode::Cover => 0,
        FitMode::Contain => 1,
        FitMode::Stretch => 2,
    });
    page.append(&labeled_row("Fit", &fit));

    page.append(&filter_heading("Smart pause — video"));
    let fs = CheckButton::with_label("Pause that monitor on exclusive fullscreen");
    fs.set_active(cfg.pause.on_fullscreen);
    let overview = CheckButton::with_label("Pause every monitor when overview is open");
    overview.set_active(cfg.pause.on_overview);
    let drag = CheckButton::with_label("Pause that monitor while dragging windows");
    drag.set_active(cfg.pause.on_window_drag);
    let covered = CheckButton::with_label("Pause that monitor when wallpaper is mostly covered");
    covered.set_active(cfg.pause.on_covered);
    page.append(&fs);
    page.append(&overview);
    page.append(&drag);
    page.append(&covered);

    page.append(&filter_heading("Smart pause — music"));
    let music_fs = CheckButton::with_label("Pause on exclusive fullscreen");
    music_fs.set_active(cfg.pause.music_on_fullscreen);
    music_fs.set_tooltip_text(Some(
        "Pauses looping wallpaper music if any monitor has exclusive fullscreen",
    ));
    let music_overview = CheckButton::with_label("Pause when overview is open");
    music_overview.set_active(cfg.pause.music_on_overview);
    let music_drag = CheckButton::with_label("Pause while dragging windows");
    music_drag.set_active(cfg.pause.music_on_window_drag);
    music_drag.set_tooltip_text(Some(
        "Pauses looping wallpaper music while windows are dragged or resized on any monitor",
    ));
    let music_covered = CheckButton::with_label("Pause when wallpaper is mostly covered");
    music_covered.set_active(cfg.pause.music_on_covered);
    music_covered.set_tooltip_text(Some(
        "Pauses looping wallpaper music if any monitor's wallpaper is mostly covered",
    ));
    let music_other_audio =
        CheckButton::with_label("Pause when other apps play sound");
    music_other_audio.set_active(cfg.pause.music_on_other_audio);
    music_other_audio.set_tooltip_text(Some(
        "Uses PulseAudio/PipeWire; ignores nwall itself and common chat/VoIP apps",
    ));
    page.append(&music_fs);
    page.append(&music_overview);
    page.append(&music_drag);
    page.append(&music_covered);
    page.append(&music_other_audio);

    page.append(&section_label("Slideshow"));
    let ss_hint = Label::new(Some(
        "Timed random wallpaper rotation from a chosen source.",
    ));
    ss_hint.set_wrap(true);
    ss_hint.set_halign(Align::Start);
    ss_hint.add_css_class("dim-label");
    page.append(&ss_hint);

    let ss_enabled = Switch::new();
    ss_enabled.set_active(cfg.slideshow.enabled);
    ss_enabled.set_halign(Align::End);
    page.append(&labeled_row("Auto-rotate", &ss_enabled));

    let ss_show_tray = Switch::new();
    ss_show_tray.set_active(cfg.slideshow.show_in_tray);
    ss_show_tray.set_halign(Align::End);
    ss_show_tray.set_tooltip_text(Some(
        "Show Start/Stop slideshow in the system tray menu.",
    ));
    page.append(&labeled_row("Show in tray", &ss_show_tray));

    let ss_interval = SpinButton::with_range(1.0, 1440.0, 1.0);
    ss_interval.set_digits(0);
    ss_interval.set_value(cfg.slideshow.interval_minutes.clamp(1, 1440) as f64);
    page.append(&labeled_row("Interval", spin_with_minutes(&ss_interval)));

    let ss_source_names = Rc::new(RefCell::new(slideshow_picker_names()));
    let ss_names_now = ss_source_names.borrow().clone();
    let ss_name_refs: Vec<&str> = ss_names_now.iter().map(|s| s.as_str()).collect();
    let ss_list = StringList::new(&ss_name_refs);
    let ss_source = DropDown::new(Some(ss_list.clone()), Option::<&gtk::Expression>::None);
    let ss_actual_source = Rc::new(RefCell::new(canonical_slideshow_source(
        &cfg.slideshow.source,
    )));
    let ss_suppress = Rc::new(Cell::new(false));
    let ss_sel = index_of_slideshow_source(&ss_source_names.borrow(), &cfg.slideshow.source)
        .unwrap_or(0) as u32;
    ss_source.set_selected(ss_sel);
    page.append(&labeled_row("Source", &ss_source));

    let ss_tags = Entry::new();
    ss_tags.set_hexpand(true);
    ss_tags.set_placeholder_text(Some("nature, mountains"));
    ss_tags.set_text(&cfg.slideshow.tags);
    page.append(&labeled_row("Tags / search", &ss_tags));

    page.append(&section_label("Interface"));

    page.append(&filter_heading("Appearance"));
    let theme_list = StringList::new(&["System", "Dark", "Light"]);
    let theme = DropDown::new(Some(theme_list), Option::<&gtk::Expression>::None);
    theme.set_selected(match cfg.theme.as_str() {
        "dark" => 1,
        "light" => 2,
        _ => 0,
    });
    page.append(&labeled_row("Theme", &theme));

    let gui_zoom = SpinButton::with_range(ZOOM_MIN * 100.0, ZOOM_MAX * 100.0, 5.0);
    gui_zoom.set_value((cfg.gui_zoom * 100.0).round());
    gui_zoom.set_digits(0);
    page.append(&labeled_row("Zoom", spin_with_percent(&gui_zoom)));

    page.append(&filter_heading("Preview"));
    let preview_hint = Label::new(Some(
        "Fast mode uses listing thumbs / a video still. Turn off for full-resolution sidebar previews (slower).",
    ));
    preview_hint.set_wrap(true);
    preview_hint.set_halign(Align::Start);
    preview_hint.add_css_class("dim-label");
    preview_hint.add_css_class("caption");
    page.append(&preview_hint);

    let show_stats = Switch::new();
    show_stats.set_active(cfg.show_stats);
    show_stats.set_halign(Align::End);
    page.append(&labeled_row("Show stats", &show_stats));

    let fast_image_preview = Switch::new();
    fast_image_preview.set_active(cfg.fast_image_preview);
    fast_image_preview.set_halign(Align::End);
    fast_image_preview.set_tooltip_text(Some(
        "On: sidebar shows the listing thumb (fast). Off: always download the full image for preview.",
    ));
    page.append(&labeled_row("Fast image preview", &fast_image_preview));

    let fast_video_preview = Switch::new();
    fast_video_preview.set_active(cfg.fast_video_preview);
    fast_video_preview.set_halign(Align::End);
    fast_video_preview.set_tooltip_text(Some(
        "On: sidebar shows a still frame (fast). Off: stream a live video preview.",
    ));
    page.append(&labeled_row("Fast video preview", &fast_video_preview));

    let preview_width = SpinButton::with_range(PREVIEW_PCT_MIN, PREVIEW_PCT_MAX, 1.0);
    preview_width.set_value(normalize_preview_width_pct(cfg.preview_width));
    preview_width.set_digits(0);
    page.append(&labeled_row("Preview bar", spin_with_percent(&preview_width)));

    page.append(&section_label("Library"));
    let library_hint = Label::new(Some("Where your wallpapers live."));
    library_hint.set_wrap(true);
    library_hint.set_halign(Align::Start);
    library_hint.add_css_class("dim-label");
    library_hint.add_css_class("caption");
    page.append(&library_hint);

    let library_entry = Entry::new();
    library_entry.set_hexpand(true);
    library_entry.set_text(&cfg.library.display().to_string());
    library_entry.set_placeholder_text(Some("~/Pictures/Wallpapers"));
    library_entry.set_tooltip_text(Some("Path to your local wallpaper folder"));

    let library_browse = Button::builder().label("Browse…").build();
    library_browse.set_tooltip_text(Some("Choose a folder"));

    let library_row = GtkBox::new(Orientation::Horizontal, 8);
    library_row.set_hexpand(true);
    library_row.append(&library_entry);
    library_row.append(&library_browse);
    page.append(&labeled_row("Folder", &library_row));

    let sources_head = GtkBox::new(Orientation::Vertical, 4);
    sources_head.append(&section_label("Wallpaper sources"));
    let src_hint = Label::new(Some(
        "Enter an API key next to a source to unlock Pixabay or Coverr, Wallhaven Sketchy/NSFW filters, higher NASA limits, or a GitHub PAT (5000 API req/hr vs 60).",
    ));
    src_hint.set_wrap(true);
    src_hint.set_halign(Align::Start);
    src_hint.add_css_class("dim-label");
    src_hint.add_css_class("caption");
    sources_head.append(&src_hint);
    page.append(&sources_head);
    let notify_sources = {
        let w = Rc::clone(&source_watchers);
        Rc::new(move || fire_source_watchers(&w)) as Rc<dyn Fn()>
    };
    let sources_list = Grid::new();
    sources_list.add_css_class("source-key-list");
    sources_list.set_column_spacing(16);
    sources_list.set_row_spacing(4);
    sources_list.set_hexpand(true);
    for (row, src) in cfg.sources.iter().enumerate() {
        attach_source_key_row(&sources_list, row as i32, src, Rc::clone(&notify_sources));
    }
    page.append(&sources_list);
    let add_caption = filter_heading("Add a GitHub repo or catalog JSON URL.");
    add_caption.set_wrap(true);
    add_caption.set_margin_top(8);
    page.append(&add_caption);
    let add_row = GtkBox::new(Orientation::Horizontal, 8);
    let src_entry = Entry::new();
    src_entry.set_hexpand(true);
    src_entry.set_placeholder_text(Some(
        "github.com/owner/repo  ·  https://…/catalog.json",
    ));
    let add_btn = Button::with_label("Add");
    add_row.append(&src_entry);
    add_row.append(&add_btn);
    page.append(&add_row);
    let add_status = Label::new(None);
    add_status.set_halign(Align::Start);
    add_status.add_css_class("dim-label");
    add_status.add_css_class("caption");
    page.append(&add_status);
    let src_entry_c = src_entry.clone();
    let add_status_c = add_status.clone();
    let notify_add = Rc::clone(&notify_sources);
    add_btn.connect_clicked(move |_| {
        let text = src_entry_c.text().to_string();
        let Some(src) = catalog::parse_added_source(&text) else {
            add_status_c.set_text("Use a GitHub repo or https://…/catalog.json URL.");
            return;
        };
        let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
        if !cfg.sources.iter().any(|s| s.name == src.name && s.url == src.url && s.repo == src.repo)
        {
            cfg.sources.push(src);
            let _ = cfg.save(&default_config_path());
        }
        src_entry_c.set_text("");
        add_status_c.set_text("");
        notify_add();
    });

    {
        let dd = ss_source.clone();
        let list = ss_list.clone();
        let names = Rc::clone(&ss_source_names);
        let actual = Rc::clone(&ss_actual_source);
        let suppress = Rc::clone(&ss_suppress);
        watch_sources(&source_watchers, move || {
            rebuild_slideshow_dropdown(&dd, &list, &names, &actual, &suppress);
        });
    }

    let root = ScrolledWindow::builder()
        .child(&page)
        .vscrollbar_policy(PolicyType::Automatic)
        .build();

    SettingsUi {
        root,
        fps,
        mute,
        volume,
        fit,
        fs,
        overview,
        drag,
        covered,
        music_fs,
        music_overview,
        music_drag,
        music_covered,
        music_other_audio,
        theme,
        library_entry,
        library_browse,
        show_stats,
        fast_image_preview,
        fast_video_preview,
        preview_width,
        gui_zoom,
        ss_enabled,
        ss_show_tray,
        ss_interval,
        ss_source,
        ss_tags,
        ss_source_names,
        ss_actual_source,
        ss_suppress,
    }
}

pub(crate) fn attach_source_key_row(
    grid: &Grid,
    row: i32,
    src: &nwall_ipc::CatalogSource,
    on_change: Rc<dyn Fn()>,
) {
    let name = Label::new(Some(&src.name));
    name.set_halign(Align::Start);
    name.set_valign(Align::Center);
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.add_css_class("heading");
    name.add_css_class("source-key-name");
    grid.attach(&name, 0, row, 1, 1);

    let kind = Label::new(Some(match src.kind.as_str() {
        "github" => "repos",
        "wallhaven" | "bing" | "nasa" => "images",
        "pixabay" | "archive" => "images & videos",
        "coverr" => "videos",
        _ => "catalog",
    }));
    kind.set_halign(Align::Start);
    kind.set_valign(Align::Center);
    kind.set_xalign(0.0);
    kind.add_css_class("dim-label");
    kind.add_css_class("source-key-kind");
    grid.attach(&kind, 1, row, 1, 1);

    if catalog::shows_api_key_entry(src) {
        let key = PasswordEntry::new();
        key.add_css_class("source-api-key");
        key.set_show_peek_icon(true);
        key.set_hexpand(true);
        key.set_halign(Align::Fill);
        key.set_valign(Align::Center);
        key.set_placeholder_text(Some(match src.kind.as_str() {
            "github" => "PAT (optional)",
            "wallhaven" | "nasa" => "API key (optional)",
            _ => "API key",
        }));
        let existing = if !src.api_key.is_empty() {
            src.api_key.clone()
        } else {
            match src.kind.as_str() {
                "coverr" => std::env::var("COVERR_API_KEY").unwrap_or_default(),
                "wallhaven" => std::env::var("WALLHAVEN_API_KEY").unwrap_or_default(),
                "nasa" => std::env::var("NASA_API_KEY").unwrap_or_default(),
                "pixabay" => std::env::var("PIXABAY_API_KEY").unwrap_or_default(),
                "github" => std::env::var("GITHUB_TOKEN")
                    .or_else(|_| std::env::var("GH_TOKEN"))
                    .unwrap_or_default(),
                _ => String::new(),
            }
        };
        if !existing.is_empty() {
            key.set_text(&existing);
        }
        let kind_s = src.kind.clone();
        let name_s = src.name.clone();
        key.connect_changed(move |e| {
            let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
            if let Some(s) = cfg
                .sources
                .iter_mut()
                .find(|s| s.kind == kind_s && s.name == name_s)
            {
                s.api_key = e.text().to_string();
                let _ = cfg.save(&default_config_path());
            }
            on_change();
        });
        grid.attach(&key, 2, row, 1, 1);
    } else {
        let slot = GtkBox::new(Orientation::Horizontal, 0);
        slot.add_css_class("source-api-key-slot");
        slot.set_hexpand(true);
        slot.set_halign(Align::Fill);
        slot.set_valign(Align::Center);
        slot.set_height_request(32);
        grid.attach(&slot, 2, row, 1, 1);
    }
}

