use std::cell::Cell;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adw::prelude::*;
use glib::object::SendWeakRef;
use gtk::{
    gdk, gio, glib, Align, Box as GtkBox, Button, ContentFit, Entry, Image, Label, ListBox,
    ListBoxRow, Orientation, Overlay, Picture, PolicyType, Scale, ScrolledWindow, SelectionMode,
    SpinButton, Spinner, Stack, StackTransitionType, Switch, Window,
};
use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use nwall_catalog::{self as catalog, MusicTrack};
use nwall_ipc::{client_request, Request};
use serde_json::Value;

use crate::ipc_util::ipc_ok;
use crate::ui::preview::refresh_bg_music_ui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MusicPicker {
    Archive,
    Youtube,
}

const ARCHIVE_CHIPS: &[(&str, &str)] = &[
    ("Ambient", "ambient"),
    ("Chill", "chill"),
    ("Piano", "piano"),
    ("Nature", "nature"),
    ("Lo-fi", "lofi OR \"lo-fi\" OR \"lo fi\""),
    ("Drone", "drone"),
];

const YT_CHIPS: &[(&str, &str)] = &[
    ("Ambient", "ambient music"),
    ("Chill", "chill music"),
    ("Piano", "piano instrumental"),
    ("Nature", "nature sounds"),
    ("Lo-fi", "lofi hip hop"),
    ("Drone", "drone ambient"),
];

impl MusicPicker {
    fn title(self) -> &'static str {
        match self {
            Self::Archive => "Archive music",
            Self::Youtube => "YouTube Music",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Archive => "Search Archive.org audio…",
            Self::Youtube => "Search YouTube Music or paste a URL…",
        }
    }

    fn source_name(self) -> &'static str {
        match self {
            Self::Archive => "Internet Archive",
            Self::Youtube => "YouTube Music",
        }
    }

    fn chips(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Archive => ARCHIVE_CHIPS,
            Self::Youtube => YT_CHIPS,
        }
    }
}

pub(crate) fn open_archive_music_dialog(
    parent: &impl IsA<gtk::Window>,
    wallpaper: PathBuf,
    status_bar: &Label,
    file_btn: &Button,
    archive_btn: &Button,
    yt_btn: &Button,
    remove_btn: &Button,
    mute: &Switch,
    vol: &SpinButton,
    mute_row: &GtkBox,
    vol_row: &GtkBox,
    name_lbl: &Label,
) {
    open_music_search_dialog(
        parent,
        wallpaper,
        status_bar,
        file_btn,
        archive_btn,
        yt_btn,
        remove_btn,
        mute,
        vol,
        mute_row,
        vol_row,
        name_lbl,
        MusicPicker::Archive,
    );
}

pub(crate) fn open_yt_music_dialog(
    parent: &impl IsA<gtk::Window>,
    wallpaper: PathBuf,
    status_bar: &Label,
    file_btn: &Button,
    archive_btn: &Button,
    yt_btn: &Button,
    remove_btn: &Button,
    mute: &Switch,
    vol: &SpinButton,
    mute_row: &GtkBox,
    vol_row: &GtkBox,
    name_lbl: &Label,
) {
    open_music_search_dialog(
        parent,
        wallpaper,
        status_bar,
        file_btn,
        archive_btn,
        yt_btn,
        remove_btn,
        mute,
        vol,
        mute_row,
        vol_row,
        name_lbl,
        MusicPicker::Youtube,
    );
}

fn open_music_search_dialog(
    parent: &impl IsA<gtk::Window>,
    wallpaper: PathBuf,
    status_bar: &Label,
    file_btn: &Button,
    archive_btn: &Button,
    yt_btn: &Button,
    remove_btn: &Button,
    mute: &Switch,
    vol: &SpinButton,
    mute_row: &GtkBox,
    vol_row: &GtkBox,
    name_lbl: &Label,
    picker: MusicPicker,
) {
    let dialog = Window::builder()
        .title(picker.title())
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .default_height(540)
        .build();

    let root = GtkBox::new(Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let search_row = GtkBox::new(Orientation::Horizontal, 6);
    let search = Entry::new();
    search.set_hexpand(true);
    search.set_placeholder_text(Some(picker.placeholder()));
    let search_btn = Button::with_label("Search");
    search_btn.add_css_class("suggested-action");
    search_row.append(&search);
    search_row.append(&search_btn);

    let chips = GtkBox::new(Orientation::Horizontal, 6);
    chips.set_halign(Align::Start);
    for (label, _) in picker.chips() {
        let chip = Button::with_label(label);
        chip.add_css_class("flat");
        chip.set_tooltip_text(Some(&format!("Browse {label} tracks")));
        chips.append(&chip);
    }

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::Single);
    list.add_css_class("boxed-list");
    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .vexpand(true)
        .min_content_height(220)
        .child(&list)
        .build();

    let dialog_status = Label::new(Some(""));
    dialog_status.set_halign(Align::Start);
    dialog_status.set_wrap(true);
    dialog_status.set_xalign(0.0);
    dialog_status.add_css_class("dim-label");

    let transport = GtkBox::new(Orientation::Horizontal, 8);
    transport.set_hexpand(true);
    let play_pause = Button::from_icon_name("media-playback-start-symbolic");
    play_pause.set_tooltip_text(Some("Play / pause preview"));
    play_pause.set_sensitive(false);
    play_pause.add_css_class("flat");
    play_pause.add_css_class("circular");
    play_pause.add_css_class("music-preview-btn");
    let time_lbl = Label::new(Some("0:00"));
    time_lbl.add_css_class("dim-label");
    time_lbl.set_width_chars(5);
    let seek = Scale::with_range(Orientation::Horizontal, 0.0, 1.0, 0.001);
    seek.set_draw_value(false);
    seek.set_hexpand(true);
    seek.set_sensitive(false);
    let dur_lbl = Label::new(Some("0:00"));
    dur_lbl.add_css_class("dim-label");
    dur_lbl.set_width_chars(5);

    let vol_icon = Button::from_icon_name("audio-volume-high-symbolic");
    vol_icon.set_tooltip_text(Some("Mute / unmute preview"));
    vol_icon.add_css_class("flat");
    vol_icon.add_css_class("circular");
    vol_icon.add_css_class("music-preview-btn");
    vol_icon.set_sensitive(false);
    let volume = Scale::with_range(Orientation::Horizontal, 0.0, 100.0, 1.0);
    volume.set_draw_value(false);
    let initial_vol = vol.value().clamp(0.0, 100.0);
    volume.set_value(if initial_vol > 0.0 { initial_vol } else { 70.0 });
    volume.set_size_request(88, -1);
    volume.set_tooltip_text(Some("Preview volume (also used when you attach the track)"));
    volume.set_sensitive(false);

    transport.append(&play_pause);
    transport.append(&time_lbl);
    transport.append(&seek);
    transport.append(&dur_lbl);
    transport.append(&vol_icon);
    transport.append(&volume);

    let use_btn = Button::with_label("Use track");
    use_btn.add_css_class("suggested-action");
    use_btn.set_sensitive(false);

    let cancel_btn = Button::with_label("Cancel");
    cancel_btn.add_css_class("flat");

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);
    actions.append(&cancel_btn);
    actions.append(&use_btn);

    root.append(&search_row);
    root.append(&chips);
    root.append(&scroll);
    root.append(&dialog_status);
    root.append(&transport);
    root.append(&actions);
    dialog.set_child(Some(&root));

    let tracks: Arc<Mutex<Vec<MusicTrack>>> = Arc::new(Mutex::new(Vec::new()));
    let search_gen = Arc::new(AtomicU64::new(0));
    let busy = Arc::new(AtomicBool::new(false));
    let player = Arc::new(Mutex::new(PreviewPlayer::new(volume.value())));
    let preview_load_gen = Arc::new(AtomicU64::new(0));
    let preview_loading_ui: Arc<Mutex<Option<LoadingRowUi>>> = Arc::new(Mutex::new(None));
    let seek_dragging = Rc::new(Cell::new(false));
    let vol_before_mute = Rc::new(Cell::new(volume.value().max(10.0)));

    {
        let use_btn = use_btn.clone();
        let busy = Arc::clone(&busy);
        list.connect_row_selected(move |_, row| {
            use_btn.set_sensitive(!busy.load(Ordering::Relaxed) && row.is_some());
        });
    }

    {
        let player = Arc::clone(&player);
        let play_pause = play_pause.clone();
        let time_lbl = time_lbl.clone();
        let dur_lbl = dur_lbl.clone();
        let seek = seek.clone();
        let volume = volume.clone();
        let vol_icon = vol_icon.clone();
        let seek_dragging = Rc::clone(&seek_dragging);
        glib::timeout_add_local(Duration::from_millis(250), move || {
            let Ok(p) = player.lock() else {
                return glib::ControlFlow::Continue;
            };
            if !p.alive() {
                set_transport_playing(&play_pause, false);
                play_pause.set_sensitive(p.has_track());
                volume.set_sensitive(p.has_track());
                vol_icon.set_sensitive(p.has_track());
                if !p.has_track() {
                    seek.set_sensitive(false);
                    seek.set_value(0.0);
                    time_lbl.set_text("0:00");
                    dur_lbl.set_text("0:00");
                }
                return glib::ControlFlow::Continue;
            }
            let paused = p.is_paused().unwrap_or(true);
            set_transport_playing(&play_pause, !paused);
            play_pause.set_sensitive(true);
            seek.set_sensitive(true);
            volume.set_sensitive(true);
            vol_icon.set_sensitive(true);
            if let Some(btn) = p.row_btn.as_ref().and_then(|w| w.upgrade()) {
                set_transport_playing(&btn, !paused);
            }
            let pos = p.time_pos().unwrap_or(0.0);
            let dur = p.duration().unwrap_or(0.0).max(0.0);
            dur_lbl.set_text(&format_clock(dur));
            if !seek_dragging.get() && dur > 0.0 {
                time_lbl.set_text(&format_clock(pos));
                seek.set_range(0.0, dur);
                seek.set_value(pos.clamp(0.0, dur));
            } else if dur > 0.0 {
                seek.set_range(0.0, dur);
            }
            update_vol_icon(&vol_icon, p.volume);
            glib::ControlFlow::Continue
        });
    }

    volume.connect_value_changed({
        let player = Arc::clone(&player);
        let vol_icon = vol_icon.clone();
        let vol_before_mute = Rc::clone(&vol_before_mute);
        move |sc| {
            let v = sc.value().clamp(0.0, 100.0);
            if v > 0.0 {
                vol_before_mute.set(v);
            }
            update_vol_icon(&vol_icon, v);
            if let Ok(mut p) = player.lock() {
                let _ = p.set_volume(v);
            }
        }
    });

    vol_icon.connect_clicked({
        let player = Arc::clone(&player);
        let volume = volume.clone();
        let vol_before_mute = Rc::clone(&vol_before_mute);
        move |_| {
            let cur = volume.value();
            if cur > 0.0 {
                vol_before_mute.set(cur);
                volume.set_value(0.0);
            } else {
                let restore = vol_before_mute.get().max(10.0);
                volume.set_value(restore);
            }
            let _ = player;
        }
    });

    play_pause.connect_clicked({
        let player = Arc::clone(&player);
        let dialog_status = dialog_status.clone();
        move |_| {
            let Ok(p) = player.lock() else {
                return;
            };
            if !p.has_track() {
                return;
            }
            match p.toggle_pause() {
                Ok(paused) => {
                    if let Some(s) = p.title.as_ref() {
                        let msg = if paused {
                            format!("Paused — {s}")
                        } else {
                            format!("Playing — {s}")
                        };
                        dialog_status.set_text(&msg);
                    }
                }
                Err(e) => dialog_status.set_text(&format!("Preview: {e:#}")),
            }
        }
    });

    {
        let player = Arc::clone(&player);
        let seek_dragging = Rc::clone(&seek_dragging);
        let time_lbl = time_lbl.clone();
        let drag_gen = Rc::new(Cell::new(0u64));
        seek.connect_change_value(move |_, _, value| {
            seek_dragging.set(true);
            time_lbl.set_text(&format_clock(value));
            if let Ok(p) = player.lock() {
                let _ = p.seek(value);
            }
            let gen = drag_gen.get().wrapping_add(1);
            drag_gen.set(gen);
            let seek_dragging = Rc::clone(&seek_dragging);
            let drag_gen = Rc::clone(&drag_gen);
            glib::timeout_add_local_once(Duration::from_millis(350), move || {
                if drag_gen.get() == gen {
                    seek_dragging.set(false);
                }
            });
            glib::Propagation::Proceed
        });
    }

    let run_search = {
        let list = list.clone();
        let tracks = Arc::clone(&tracks);
        let dialog_status = dialog_status.clone();
        let search_btn = search_btn.clone();
        let use_btn = use_btn.clone();
        let search_entry = search.clone();
        let search_gen = Arc::clone(&search_gen);
        let busy = Arc::clone(&busy);
        let player = Arc::clone(&player);
        let chips_sensitive = chips.clone();
        let play_pause = play_pause.clone();
        let preview_load_gen = Arc::clone(&preview_load_gen);
        let preview_loading_ui = Arc::clone(&preview_loading_ui);
        Rc::new(move |query: String, status_prefix: Option<String>| {
            if busy.load(Ordering::Relaxed) {
                return;
            }
            preview_load_gen.fetch_add(1, Ordering::Relaxed);
            clear_loading_row(&preview_loading_ui);
            {
                let Ok(mut p) = player.lock() else {
                    return;
                };
                p.stop();
            }
            set_transport_playing(&play_pause, false);
            play_pause.set_sensitive(false);
            let my = search_gen.fetch_add(1, Ordering::Relaxed) + 1;
            busy.store(true, Ordering::Relaxed);
            search_btn.set_sensitive(false);
            search_entry.set_sensitive(false);
            use_btn.set_sensitive(false);
            set_chips_sensitive(&chips_sensitive, false);
            dialog_status.set_text(status_prefix.as_deref().unwrap_or("Searching…"));
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            if let Ok(mut t) = tracks.lock() {
                t.clear();
            }

            let list_w = SendWeakRef::from(list.downgrade());
            let status_w = SendWeakRef::from(dialog_status.downgrade());
            let use_w = SendWeakRef::from(use_btn.downgrade());
            let search_btn_w = SendWeakRef::from(search_btn.downgrade());
            let search_w = SendWeakRef::from(search_entry.downgrade());
            let chips_w = SendWeakRef::from(chips_sensitive.downgrade());
            let tracks_c = Arc::clone(&tracks);
            let gen = Arc::clone(&search_gen);
            let busy_c = Arc::clone(&busy);
            let player_c = Arc::clone(&player);
            let preview_loading_ui_c = Arc::clone(&preview_loading_ui);
            let preview_load_gen_c = Arc::clone(&preview_load_gen);
            std::thread::spawn(move || {
                let result = fetch_picker_music(picker, &query);
                glib::idle_add_once(move || {
                    if gen.load(Ordering::Relaxed) != my {
                        return;
                    }
                    busy_c.store(false, Ordering::Relaxed);
                    let can_search = picker_search_ok(picker);
                    if let Some(b) = search_btn_w.upgrade() {
                        b.set_sensitive(can_search);
                    }
                    if let Some(e) = search_w.upgrade() {
                        e.set_sensitive(can_search);
                    }
                    if let Some(c) = chips_w.upgrade() {
                        set_chips_sensitive(&c, can_search);
                    }
                    let Some(list) = list_w.upgrade() else {
                        return;
                    };
                    let Some(status) = status_w.upgrade() else {
                        return;
                    };
                    match result {
                        Ok(found) if found.is_empty() && picker == MusicPicker::Archive => {
                            let starters = catalog::archive_music_starters();
                            fill_track_list(
                                &list,
                                &tracks_c,
                                &starters,
                                &player_c,
                                &status,
                                &busy_c,
                                &preview_load_gen_c,
                                &preview_loading_ui_c,
                                picker,
                            );
                            status.set_text("No search hits — showing starters");
                            if let Some(u) = use_w.upgrade() {
                                u.set_sensitive(list.selected_row().is_some());
                            }
                        }
                        Ok(found) if found.is_empty() => {
                            fill_track_list(
                                &list,
                                &tracks_c,
                                &found,
                                &player_c,
                                &status,
                                &busy_c,
                                &preview_load_gen_c,
                                &preview_loading_ui_c,
                                picker,
                            );
                            status.set_text("No tracks found");
                            if let Some(u) = use_w.upgrade() {
                                u.set_sensitive(false);
                            }
                        }
                        Ok(found) => {
                            let n = found.len();
                            fill_track_list(
                                &list,
                                &tracks_c,
                                &found,
                                &player_c,
                                &status,
                                &busy_c,
                                &preview_load_gen_c,
                                &preview_loading_ui_c,
                                picker,
                            );
                            status.set_text(&format!("{n} tracks"));
                            if let Some(u) = use_w.upgrade() {
                                u.set_sensitive(list.selected_row().is_some());
                            }
                        }
                        Err(e) if picker == MusicPicker::Archive => {
                            let starters = catalog::archive_music_starters();
                            fill_track_list(
                                &list,
                                &tracks_c,
                                &starters,
                                &player_c,
                                &status,
                                &busy_c,
                                &preview_load_gen_c,
                                &preview_loading_ui_c,
                                picker,
                            );
                            status.set_text(&format!(
                                "Archive.org timed out — starters ready ({})",
                                short_err(&e)
                            ));
                            if let Some(u) = use_w.upgrade() {
                                u.set_sensitive(list.selected_row().is_some());
                            }
                        }
                        Err(e) => {
                            fill_track_list(
                                &list,
                                &tracks_c,
                                &[],
                                &player_c,
                                &status,
                                &busy_c,
                                &preview_load_gen_c,
                                &preview_loading_ui_c,
                                picker,
                            );
                            status.set_text(&short_err(&e));
                            if let Some(u) = use_w.upgrade() {
                                u.set_sensitive(false);
                            }
                        }
                    }
                });
            });
        })
    };

    {
        let mut child = chips.first_child();
        let mut i = 0usize;
        while let Some(w) = child {
            let next = w.next_sibling();
            if let Ok(btn) = w.downcast::<Button>() {
                if let Some((label, q)) = picker.chips().get(i) {
                    let run_search = Rc::clone(&run_search);
                    let search = search.clone();
                    let query = (*q).to_string();
                    let chip_label = (*label).to_string();
                    btn.connect_clicked(move |_| {
                        search.set_text(&query);
                        run_search(query.clone(), Some(format!("Loading {chip_label}…")));
                    });
                }
                i += 1;
            }
            child = next;
        }
    }

    search_btn.connect_clicked({
        let run_search = Rc::clone(&run_search);
        let search = search.clone();
        move |_| run_search(search.text().to_string(), None)
    });
    search.connect_activate({
        let run_search = Rc::clone(&run_search);
        let search = search.clone();
        move |_| run_search(search.text().to_string(), None)
    });

    {
        let player = Arc::clone(&player);
        dialog.connect_close_request(move |_| {
            if let Ok(mut p) = player.lock() {
                p.stop();
            }
            glib::Propagation::Proceed
        });
    }

    cancel_btn.connect_clicked({
        let dialog = dialog.clone();
        let player = Arc::clone(&player);
        move |_| {
            if let Ok(mut p) = player.lock() {
                p.stop();
            }
            dialog.close();
        }
    });

    let wallpaper_c = wallpaper;
    let status_bar = status_bar.clone();
    let file_btn = file_btn.clone();
    let archive_btn = archive_btn.clone();
    let yt_btn = yt_btn.clone();
    let remove_btn = remove_btn.clone();
    let mute = mute.clone();
    let vol = vol.clone();
    let mute_row = mute_row.clone();
    let vol_row = vol_row.clone();
    let name_lbl = name_lbl.clone();
    let list_u = list.clone();
    let tracks_u = Arc::clone(&tracks);
    let dialog_status_u = dialog_status.clone();
    let dialog_c = dialog.clone();
    let search_btn_u = search_btn.clone();
    let search_u = search.clone();
    let use_btn_u = use_btn.clone();
    let busy_u = Arc::clone(&busy);
    let player_u = Arc::clone(&player);
    let chips_u = chips.clone();
    let play_pause_u = play_pause.clone();
    let preview_load_gen_u = Arc::clone(&preview_load_gen);
    let preview_loading_ui_u = Arc::clone(&preview_loading_ui);
    let preview_volume = volume.clone();

    use_btn.connect_clicked(move |_| {
        if busy_u.load(Ordering::Relaxed) {
            return;
        }
        let Some(row) = list_u.selected_row() else {
            dialog_status_u.set_text("Select a track first");
            return;
        };
        let idx = row.index() as usize;
        let track = {
            let Ok(guard) = tracks_u.lock() else {
                dialog_status_u.set_text("Select a track first");
                return;
            };
            let Some(t) = guard.get(idx).cloned() else {
                dialog_status_u.set_text("Select a track first");
                return;
            };
            t
        };
        if let Ok(mut p) = player_u.lock() {
            p.stop();
        }
        preview_load_gen_u.fetch_add(1, Ordering::Relaxed);
        clear_loading_row(&preview_loading_ui_u);
        set_transport_playing(&play_pause_u, false);
        play_pause_u.set_sensitive(false);
        busy_u.store(true, Ordering::Relaxed);
        search_btn_u.set_sensitive(false);
        search_u.set_sensitive(false);
        use_btn_u.set_sensitive(false);
        set_chips_sensitive(&chips_u, false);
        dialog_status_u.set_text(&format!("Downloading {}…", track.title));

        let wall = wallpaper_c.clone();
        let preview_pct = preview_volume.value().clamp(0.0, 100.0);
        let volume = (preview_pct as f32) / 100.0;
        vol.set_value(preview_pct);
        let dialog_status_w = SendWeakRef::from(dialog_status_u.downgrade());
        let dialog_w = SendWeakRef::from(dialog_c.downgrade());
        let status_bar_w = SendWeakRef::from(status_bar.downgrade());
        let file_btn_w = SendWeakRef::from(file_btn.downgrade());
        let archive_btn_w = SendWeakRef::from(archive_btn.downgrade());
        let yt_btn_w = SendWeakRef::from(yt_btn.downgrade());
        let remove_btn_w = SendWeakRef::from(remove_btn.downgrade());
        let mute_w = SendWeakRef::from(mute.downgrade());
        let vol_w = SendWeakRef::from(vol.downgrade());
        let mute_row_w = SendWeakRef::from(mute_row.downgrade());
        let vol_row_w = SendWeakRef::from(vol_row.downgrade());
        let name_w = SendWeakRef::from(name_lbl.downgrade());
        let search_btn_w = SendWeakRef::from(search_btn_u.downgrade());
        let search_w = SendWeakRef::from(search_u.downgrade());
        let use_btn_w = SendWeakRef::from(use_btn_u.downgrade());
        let list_w = SendWeakRef::from(list_u.downgrade());
        let chips_w = SendWeakRef::from(chips_u.downgrade());
        let busy_c = Arc::clone(&busy_u);
        let title = track.title.clone();

        std::thread::spawn(move || {
            let source_name = picker.source_name();
            let result = match download_picker_music(picker, &track) {
                Ok(path) => {
                    let mut meta = catalog::load_path_meta(&path).unwrap_or_default();
                    if meta
                        .title
                        .as_ref()
                        .map(|s| s.trim().is_empty())
                        .unwrap_or(true)
                    {
                        meta.title = Some(track.title.clone());
                    }
                    if meta
                        .credit
                        .as_ref()
                        .map(|s| s.trim().is_empty())
                        .unwrap_or(true)
                    {
                        meta.credit = track
                            .creator
                            .clone()
                            .filter(|s| !s.trim().is_empty())
                            .or_else(|| Some(source_name.into()));
                    }
                    if meta
                        .page_url
                        .as_ref()
                        .map(|s| s.trim().is_empty())
                        .unwrap_or(true)
                    {
                        meta.page_url = Some(track.page_url.clone());
                    }
                    if meta
                        .source
                        .as_ref()
                        .map(|s| s.trim().is_empty())
                        .unwrap_or(true)
                    {
                        meta.source = Some(source_name.into());
                    }
                    catalog::persist_path_meta(&path, &meta);
                    Ok(path)
                }
                Err(e) => Err(e),
            };

            glib::idle_add_once(move || {
                let reenable = |selected: bool| {
                    busy_c.store(false, Ordering::Relaxed);
                    let can_search = picker_search_ok(picker);
                    if let Some(b) = search_btn_w.upgrade() {
                        b.set_sensitive(can_search);
                    }
                    if let Some(e) = search_w.upgrade() {
                        e.set_sensitive(can_search);
                    }
                    if let Some(c) = chips_w.upgrade() {
                        set_chips_sensitive(&c, can_search);
                    }
                    if let Some(u) = use_btn_w.upgrade() {
                        u.set_sensitive(selected);
                    }
                };
                match result {
                    Ok(path) => {
                        match client_request(&Request::SetBgMusic {
                            wallpaper: wall.clone(),
                            music: Some(path),
                        })
                        .and_then(ipc_ok)
                        {
                            Ok(()) => {
                                let _ = client_request(&Request::SetBgMusicVolume {
                                    wallpaper: wall.clone(),
                                    volume,
                                });
                                let _ = client_request(&Request::SetBgMusicMute {
                                    wallpaper: Some(wall.clone()),
                                    mute: volume <= 0.001,
                                });
                                let suppress = Rc::new(Cell::new(false));
                                if let (
                                    Some(file_btn),
                                    Some(archive_btn),
                                    Some(yt_btn),
                                    Some(remove_btn),
                                    Some(mute),
                                    Some(vol),
                                    Some(mute_row),
                                    Some(vol_row),
                                    Some(name_lbl),
                                ) = (
                                    file_btn_w.upgrade(),
                                    archive_btn_w.upgrade(),
                                    yt_btn_w.upgrade(),
                                    remove_btn_w.upgrade(),
                                    mute_w.upgrade(),
                                    vol_w.upgrade(),
                                    mute_row_w.upgrade(),
                                    vol_row_w.upgrade(),
                                    name_w.upgrade(),
                                ) {
                                    refresh_bg_music_ui(
                                        Some(&wall),
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
                                }
                                if let Some(s) = status_bar_w.upgrade() {
                                    s.set_text(&format!("Background music: {title}"));
                                }
                                if let Some(d) = dialog_w.upgrade() {
                                    d.close();
                                }
                            }
                            Err(e) => {
                                if let Some(st) = dialog_status_w.upgrade() {
                                    st.set_text(&format!("Music failed: {e:#}"));
                                }
                                if let Some(s) = status_bar_w.upgrade() {
                                    s.set_text(&format!("Music failed: {e:#}"));
                                }
                                let selected = list_w
                                    .upgrade()
                                    .map(|l| l.selected_row().is_some())
                                    .unwrap_or(false);
                                reenable(selected);
                            }
                        }
                    }
                    Err(e) => {
                        let msg = format!("Download failed: {e:#}");
                        if let Some(st) = dialog_status_w.upgrade() {
                            st.set_text(&msg);
                        }
                        if let Some(s) = status_bar_w.upgrade() {
                            s.set_text(&msg);
                        }
                        let selected = list_w
                            .upgrade()
                            .map(|l| l.selected_row().is_some())
                            .unwrap_or(false);
                        reenable(selected);
                    }
                }
            });
        });
    });

    dialog.present();

    let can_search = picker_search_ok(picker);
    search.set_sensitive(can_search);
    search_btn.set_sensitive(can_search);
    set_chips_sensitive(&chips, can_search);
    if picker == MusicPicker::Youtube {
        fill_track_list(
            &list,
            &tracks,
            &[],
            &player,
            &dialog_status,
            &busy,
            &preview_load_gen,
            &preview_loading_ui,
            picker,
        );
        dialog_status.set_text("Search YouTube Music or pick a mood");
    } else {
        let starters = catalog::archive_music_starters();
        fill_track_list(
            &list,
            &tracks,
            &starters,
            &player,
            &dialog_status,
            &busy,
            &preview_load_gen,
            &preview_loading_ui,
            picker,
        );
        dialog_status.set_text(&format!("{} tracks", starters.len()));
    }
    search.set_text("");
}

struct LoadingRowUi {
    gen: u64,
    stack: SendWeakRef<Stack>,
    play: SendWeakRef<Button>,
    spinner: SendWeakRef<Spinner>,
}

fn set_transport_playing(btn: &Button, playing: bool) {
    btn.set_icon_name(if playing {
        "media-playback-pause-symbolic"
    } else {
        "media-playback-start-symbolic"
    });
}

fn update_vol_icon(btn: &Button, volume: f64) {
    let name = if volume <= 0.0 {
        "audio-volume-muted-symbolic"
    } else if volume < 33.0 {
        "audio-volume-low-symbolic"
    } else if volume < 66.0 {
        "audio-volume-medium-symbolic"
    } else {
        "audio-volume-high-symbolic"
    };
    btn.set_icon_name(name);
}

fn show_row_play(stack: &Stack, spinner: &Spinner) {
    spinner.set_spinning(false);
    stack.set_visible_child_name("play");
}

fn show_row_spinner(stack: &Stack, spinner: &Spinner) {
    stack.set_visible_child_name("spin");
    spinner.set_spinning(true);
}

fn clear_loading_row(slot: &Arc<Mutex<Option<LoadingRowUi>>>) {
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    if let Some(ui) = guard.take() {
        if let Some(s) = ui.spinner.upgrade() {
            s.set_spinning(false);
        }
        if let Some(stack) = ui.stack.upgrade() {
            stack.set_visible_child_name("play");
        }
        if let Some(p) = ui.play.upgrade() {
            set_transport_playing(&p, false);
        }
    }
}

fn short_err(e: &anyhow::Error) -> String {
    let s = e.root_cause().to_string();
    if s.contains("YouTube blocked this stream")
        || s.contains("Sign in to confirm")
        || s.contains("not a bot")
        || s.contains("LOGIN_REQUIRED")
        || s.contains("ERROR: [youtube]")
    {
        "YouTube blocked this stream".into()
    } else if s.contains("install yt-dlp") || s.contains("yt-dlp not found") {
        "paste a video URL or search by name".into()
    } else if s.contains("timed out") || s.contains("timeout") {
        "timed out — try again".into()
    } else if s.contains("cancelled") {
        "cancelled".into()
    } else if s.contains("got text/html") || s.contains("instead of audio") {
        "Archive.org busy — try again".into()
    } else if s.contains("truncated") {
        "download truncated — try again".into()
    } else if s.contains("no suitable audio") {
        "no playable audio in this item".into()
    } else if s.contains("preview player") || s.contains("mpv") {
        "player failed — is mpv installed?".into()
    } else if s.len() > 70 {
        format!("{}…", s.chars().take(67).collect::<String>())
    } else {
        s
    }
}

fn format_clock(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".into();
    }
    let total = secs.floor() as u64;
    let m = total / 60;
    let s = total % 60;
    if m >= 60 {
        let h = m / 60;
        let m = m % 60;
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn picker_search_ok(_picker: MusicPicker) -> bool {
    true
}

fn fetch_picker_music(picker: MusicPicker, query: &str) -> anyhow::Result<Vec<MusicTrack>> {
    match picker {
        MusicPicker::Archive => catalog::fetch_archive_music(query, 1),
        MusicPicker::Youtube => catalog::fetch_yt_music(query),
    }
}

fn download_picker_music(picker: MusicPicker, track: &MusicTrack) -> anyhow::Result<PathBuf> {
    match picker {
        MusicPicker::Archive => catalog::download_archive_music(track),
        MusicPicker::Youtube => catalog::download_yt_music(track),
    }
}

fn preview_picker_music(picker: MusicPicker, track: &MusicTrack) -> anyhow::Result<String> {
    match picker {
        MusicPicker::Archive => {
            download_picker_music(picker, track).map(|p| p.to_string_lossy().into_owned())
        }
        MusicPicker::Youtube => {
            let media = catalog::resolve_yt_music_audio(track)?;
            if !(media.starts_with("http://") || media.starts_with("https://")) {
                return Ok(media);
            }
            let track = track.clone();
            std::thread::spawn(move || {
                catalog::prefetch_yt_music(&track);
            });
            Ok(media)
        }
    }
}

fn fill_track_list(
    list: &ListBox,
    tracks: &Arc<Mutex<Vec<MusicTrack>>>,
    found: &[MusicTrack],
    player: &Arc<Mutex<PreviewPlayer>>,
    status: &Label,
    busy: &Arc<AtomicBool>,
    preview_load_gen: &Arc<AtomicU64>,
    preview_loading_ui: &Arc<Mutex<Option<LoadingRowUi>>>,
    picker: MusicPicker,
) {
    clear_loading_row(preview_loading_ui);
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    if let Ok(mut t) = tracks.lock() {
        *t = found.to_vec();
    }
    for (idx, track) in found.iter().enumerate() {
        list.append(&make_track_row(
            track,
            idx,
            tracks,
            player,
            status,
            busy,
            preview_load_gen,
            preview_loading_ui,
            picker,
        ));
    }
}

fn set_chips_sensitive(chips: &GtkBox, sensitive: bool) {
    let mut child = chips.first_child();
    while let Some(w) = child {
        let next = w.next_sibling();
        if let Ok(btn) = w.downcast::<Button>() {
            btn.set_sensitive(sensitive);
        }
        child = next;
    }
}

fn make_track_row(
    track: &MusicTrack,
    idx: usize,
    tracks: &Arc<Mutex<Vec<MusicTrack>>>,
    player: &Arc<Mutex<PreviewPlayer>>,
    status: &Label,
    busy: &Arc<AtomicBool>,
    preview_load_gen: &Arc<AtomicU64>,
    preview_loading_ui: &Arc<Mutex<Option<LoadingRowUi>>>,
    picker: MusicPicker,
) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.add_css_class("music-track-row");

    let outer = GtkBox::new(Orientation::Horizontal, 8);
    outer.set_margin_start(6);
    outer.set_margin_end(4);
    outer.set_margin_top(4);
    outer.set_margin_bottom(4);

    let (cover_host, cover, cover_ph) = make_music_cover();
    let cover_url = track
        .thumb_url
        .clone()
        .unwrap_or_else(|| format!("https://archive.org/services/img/{}", track.id.trim()));
    bind_music_cover(&cover, &cover_ph, &cover_url);

    let text = GtkBox::new(Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_halign(Align::Fill);
    text.set_valign(Align::Center);

    let title = Label::new(Some(&track.title));
    title.set_halign(Align::Start);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_xalign(0.0);
    title.add_css_class("music-track-title");

    let creator_row = GtkBox::new(Orientation::Horizontal, 4);
    creator_row.set_halign(Align::Start);
    let creator_name = track
        .creator
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(picker.source_name());
    let avatar = adw::Avatar::new(MUSIC_AVATAR_PX, Some(creator_name), true);
    avatar.set_valign(Align::Center);
    avatar.set_tooltip_text(Some(creator_name));
    avatar.add_css_class("uploader-avatar");
    bind_music_avatar(&avatar, track.avatar.as_deref());
    if picker == MusicPicker::Youtube
        && track
            .avatar
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
    {
        let track_c = track.clone();
        let id = track.id.clone();
        let av_w = SendWeakRef::from(avatar.downgrade());
        let tracks_c = Arc::clone(tracks);
        std::thread::spawn(move || {
            let Some(url) = catalog::fetch_yt_music_avatar(&track_c) else {
                return;
            };
            glib::idle_add_once(move || {
                if let Ok(mut guard) = tracks_c.lock() {
                    if let Some(t) = guard.get_mut(idx) {
                        if t.id == id {
                            t.avatar = Some(url.clone());
                        }
                    }
                }
                if let Some(av) = av_w.upgrade() {
                    bind_music_avatar(&av, Some(&url));
                }
            });
        });
    }
    let creator_lbl = Label::new(Some(creator_name));
    creator_lbl.set_halign(Align::Start);
    creator_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
    creator_lbl.set_xalign(0.0);
    creator_lbl.add_css_class("dim-label");
    creator_lbl.add_css_class("caption");
    creator_row.append(&avatar);
    creator_row.append(&creator_lbl);

    text.append(&title);
    text.append(&creator_row);

    let dur_lbl = Label::new(None);
    dur_lbl.add_css_class("dim-label");
    dur_lbl.add_css_class("caption");
    dur_lbl.set_width_chars(5);
    dur_lbl.set_xalign(1.0);
    dur_lbl.set_valign(Align::Center);
    if let Some(d) = track.duration.filter(|d| d.is_finite() && *d > 0.0) {
        let formatted = catalog::format_duration(d);
        if !formatted.is_empty() {
            dur_lbl.set_text(&formatted);
            dur_lbl.set_visible(true);
        } else {
            dur_lbl.set_visible(false);
        }
    } else if picker == MusicPicker::Archive {
        dur_lbl.set_visible(false);
        let id = track.id.clone();
        let dur_w = SendWeakRef::from(dur_lbl.downgrade());
        let tracks_c = Arc::clone(tracks);
        std::thread::spawn(move || {
            let Some(d) = catalog::fetch_archive_music_duration(&id) else {
                return;
            };
            glib::idle_add_once(move || {
                let formatted = catalog::format_duration(d);
                if formatted.is_empty() {
                    return;
                }
                if let Some(lbl) = dur_w.upgrade() {
                    lbl.set_text(&formatted);
                    lbl.set_visible(true);
                }
                if let Ok(mut guard) = tracks_c.lock() {
                    if let Some(t) = guard.get_mut(idx) {
                        if t.id == id {
                            t.duration = Some(d);
                        }
                    }
                }
            });
        });
    } else {
        dur_lbl.set_visible(false);
    }

    let play = Button::from_icon_name("media-playback-start-symbolic");
    play.set_tooltip_text(Some("Play / pause"));
    play.set_valign(Align::Center);
    play.add_css_class("flat");
    play.add_css_class("circular");
    play.add_css_class("music-preview-btn");
    play.set_focus_on_click(false);

    let spinner = Spinner::new();
    spinner.set_halign(Align::Center);
    spinner.set_valign(Align::Center);
    spinner.set_size_request(14, 14);

    let stack = Stack::new();
    stack.set_transition_type(StackTransitionType::None);
    stack.set_valign(Align::Center);
    stack.add_css_class("music-preview-ctrl");
    stack.add_named(&play, Some("play"));
    stack.add_named(&spinner, Some("spin"));
    stack.set_visible_child_name("play");

    {
        let tracks = Arc::clone(tracks);
        let player = Arc::clone(player);
        let status = status.clone();
        let busy = Arc::clone(busy);
        let play_btn = play.clone();
        let spinner = spinner.clone();
        let stack = stack.clone();
        let dur_lbl_c = dur_lbl.clone();
        let preview_load_gen = Arc::clone(preview_load_gen);
        let preview_loading_ui = Arc::clone(preview_loading_ui);
        play.connect_clicked(move |btn| {
            if let Some(row) = btn
                .ancestor(ListBoxRow::static_type())
                .and_then(|w| w.downcast::<ListBoxRow>().ok())
            {
                if let Some(list) = row.parent().and_then(|p| p.downcast::<ListBox>().ok()) {
                    list.select_row(Some(&row));
                }
            }
            if busy.load(Ordering::Relaxed) {
                return;
            }

            {
                let Ok(p) = player.lock() else {
                    return;
                };
                if p.track_idx == Some(idx) && p.has_track() {
                    match p.toggle_pause() {
                        Ok(paused) => {
                            set_transport_playing(btn, !paused);
                            if let Some(t) = p.title.as_ref() {
                                let msg = if paused {
                                    format!("Paused — {t}")
                                } else {
                                    format!("Playing — {t}")
                                };
                                status.set_text(&msg);
                            }
                        }
                        Err(e) => status.set_text(&format!("Preview: {e:#}")),
                    }
                    return;
                }
            }

            let my = preview_load_gen.fetch_add(1, Ordering::Relaxed) + 1;
            clear_loading_row(&preview_loading_ui);
            if let Ok(mut p) = player.lock() {
                p.stop();
            }

            let track = {
                let Ok(guard) = tracks.lock() else {
                    return;
                };
                let Some(t) = guard.get(idx).cloned() else {
                    return;
                };
                t
            };
            let title = track.title.clone();
            status.set_text(&format!("Loading {title}…"));

            show_row_spinner(&stack, &spinner);
            if let Ok(mut slot) = preview_loading_ui.lock() {
                *slot = Some(LoadingRowUi {
                    gen: my,
                    stack: SendWeakRef::from(stack.downgrade()),
                    play: SendWeakRef::from(play_btn.downgrade()),
                    spinner: SendWeakRef::from(spinner.downgrade()),
                });
            }

            let status_w = SendWeakRef::from(status.downgrade());
            let play_w = SendWeakRef::from(play_btn.downgrade());
            let spin_w = SendWeakRef::from(spinner.downgrade());
            let stack_w = SendWeakRef::from(stack.downgrade());
            let dur_w = SendWeakRef::from(dur_lbl_c.downgrade());
            let player_c = Arc::clone(&player);
            let gen = Arc::clone(&preview_load_gen);
            let loading_ui = Arc::clone(&preview_loading_ui);
            let tracks_c = Arc::clone(&tracks);
            std::thread::spawn(move || {
                let result = if gen.load(Ordering::Relaxed) != my {
                    Err(anyhow::anyhow!("cancelled"))
                } else {
                    preview_picker_music(picker, &track).and_then(|media| {
                        if gen.load(Ordering::Relaxed) != my {
                            return Err(anyhow::anyhow!("cancelled"));
                        }
                        let mut p = player_c
                            .lock()
                            .map_err(|_| anyhow::anyhow!("preview lock"))?;
                        if gen.load(Ordering::Relaxed) != my {
                            return Err(anyhow::anyhow!("cancelled"));
                        }
                        p.load_media(idx, &title, &media, play_w.clone())?;
                        let dur = p.duration().ok().filter(|d| *d > 0.0);
                        Ok((title, dur))
                    })
                };
                glib::idle_add_once(move || {
                    if gen.load(Ordering::Relaxed) != my {
                        return;
                    }
                    if let Ok(mut slot) = loading_ui.lock() {
                        if slot.as_ref().map(|u| u.gen) == Some(my) {
                            slot.take();
                        }
                    }
                    if let (Some(stack), Some(spin)) = (stack_w.upgrade(), spin_w.upgrade()) {
                        show_row_play(&stack, &spin);
                    }
                    match result {
                        Ok((title, dur)) => {
                            if let Some(b) = play_w.upgrade() {
                                set_transport_playing(&b, true);
                            }
                            if let (Some(d), Some(lbl)) = (dur, dur_w.upgrade()) {
                                let formatted = catalog::format_duration(d);
                                if !formatted.is_empty() {
                                    lbl.set_text(&formatted);
                                    lbl.set_visible(true);
                                }
                                if let Ok(mut guard) = tracks_c.lock() {
                                    if let Some(t) = guard.get_mut(idx) {
                                        t.duration = Some(d);
                                    }
                                }
                            }
                            if let Some(s) = status_w.upgrade() {
                                s.set_text(&format!("Playing — {title}"));
                            }
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            if msg.contains("cancelled") {
                                return;
                            }
                            if let Some(b) = play_w.upgrade() {
                                set_transport_playing(&b, false);
                            }
                            if let Some(s) = status_w.upgrade() {
                                s.set_text(&format!("Preview failed: {}", short_err(&e)));
                            }
                        }
                    }
                });
            });
        });
    }

    outer.append(&cover_host);
    outer.append(&text);
    outer.append(&dur_lbl);
    outer.append(&stack);
    row.set_child(Some(&outer));
    row
}

const MUSIC_COVER_PX: i32 = 36;
const MUSIC_AVATAR_PX: i32 = 16;
const YT_PREVIEW_UA: &str = "com.google.android.youtube/21.26.364 (Linux; U; Android 11) gzip";

fn make_music_cover() -> (Overlay, Picture, Image) {
    let pic = Picture::new();
    pic.set_content_fit(ContentFit::Cover);
    pic.set_can_shrink(true);
    pic.set_halign(Align::Fill);
    pic.set_valign(Align::Fill);
    pic.add_css_class("music-cover");

    let placeholder = Image::from_icon_name("audio-x-generic-symbolic");
    placeholder.set_pixel_size(18);
    placeholder.set_halign(Align::Center);
    placeholder.set_valign(Align::Center);
    placeholder.add_css_class("dim-label");
    placeholder.add_css_class("music-cover-ph");

    let driver = GtkBox::new(Orientation::Vertical, 0);
    driver.set_size_request(MUSIC_COVER_PX, MUSIC_COVER_PX);
    driver.set_halign(Align::Fill);
    driver.set_valign(Align::Fill);
    driver.add_css_class("music-cover-ph-bg");

    let host = Overlay::new();
    host.set_size_request(MUSIC_COVER_PX, MUSIC_COVER_PX);
    host.set_hexpand(false);
    host.set_vexpand(false);
    host.set_halign(Align::Center);
    host.set_valign(Align::Center);
    host.set_overflow(gtk::Overflow::Hidden);
    host.add_css_class("music-cover-wrap");
    host.set_child(Some(&driver));
    host.add_overlay(&placeholder);
    host.add_overlay(&pic);
    // set_measure_overlay(false): overlays must not expand row height.
    host.set_measure_overlay(&placeholder, false);
    host.set_measure_overlay(&pic, false);
    host.set_clip_overlay(&pic, true);
    (host, pic, placeholder)
}

fn bind_music_avatar(avatar: &adw::Avatar, url: Option<&str>) {
    let Some(url) = url.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let dest = catalog::cached_path(url, "thumb");
    if dest.is_file() && dest.metadata().map(|m| m.len() > 24).unwrap_or(false) {
        set_music_avatar_from_path(avatar, &dest);
        return;
    }
    let url = url.to_string();
    let weak = SendWeakRef::from(avatar.downgrade());
    std::thread::spawn(move || {
        let Ok(path) = catalog::download_thumb(&url) else {
            return;
        };
        if path.metadata().map(|m| m.len() < 24).unwrap_or(true) {
            return;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            if bytes.starts_with(b"<") || bytes.starts_with(b"<!") {
                let _ = std::fs::remove_file(&path);
                return;
            }
        }
        glib::idle_add_once(move || {
            if let Some(av) = weak.upgrade() {
                set_music_avatar_from_path(&av, &path);
            }
        });
    });
}

fn set_music_avatar_from_path(avatar: &adw::Avatar, path: &Path) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    if bytes.len() < 24 || bytes.starts_with(b"<") || bytes.starts_with(b"<!") {
        return;
    }
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from(&bytes));
    let Ok(pb) = gdk_pixbuf::Pixbuf::from_stream(&stream, gio::Cancellable::NONE) else {
        return;
    };
    let side = MUSIC_AVATAR_PX * 2;
    let pb = pb
        .scale_simple(side, side, gdk_pixbuf::InterpType::Bilinear)
        .unwrap_or(pb);
    avatar.set_custom_image(Some(&gdk::Texture::for_pixbuf(&pb)));
    avatar.set_visible(true);
}

fn bind_music_cover(pic: &Picture, placeholder: &Image, url: &str) {
    let url = url.trim().to_string();
    if url.is_empty() {
        return;
    }
    let dest = catalog::cached_path(&url, "thumb");
    if dest.is_file() && dest.metadata().map(|m| m.len() > 64).unwrap_or(false) {
        if set_music_cover_from_path(pic, &dest) {
            placeholder.set_visible(false);
        }
        return;
    }
    let weak = SendWeakRef::from(pic.downgrade());
    let ph_w = SendWeakRef::from(placeholder.downgrade());
    std::thread::spawn(move || {
        let Ok(path) = catalog::download_thumb(&url) else {
            return;
        };
        if path.metadata().map(|m| m.len() < 64).unwrap_or(true) {
            return;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            if bytes.starts_with(b"<") || bytes.starts_with(b"<!") {
                let _ = std::fs::remove_file(&path);
                return;
            }
        }
        glib::idle_add_once(move || {
            let Some(pic) = weak.upgrade() else {
                return;
            };
            if set_music_cover_from_path(&pic, &path) {
                if let Some(ph) = ph_w.upgrade() {
                    ph.set_visible(false);
                }
            }
        });
    });
}

fn set_music_cover_from_path(pic: &Picture, path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    if bytes.len() < 64 || bytes.starts_with(b"<") || bytes.starts_with(b"<!") {
        return false;
    }
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from(&bytes));
    let Ok(pb) = gdk_pixbuf::Pixbuf::from_stream(&stream, gio::Cancellable::NONE) else {
        return false;
    };
    let w = pb.width();
    let h = pb.height();
    if w < 8 || h < 8 {
        return false;
    }
    let ratio = w as f64 / h as f64;
    if !(0.45..=2.4).contains(&ratio) {
        return false;
    }
    let side = w.min(h);
    let x = (w - side) / 2;
    let y = (h - side) / 2;
    let square = pb.new_subpixbuf(x, y, side, side);
    let scaled = square
        .scale_simple(
            MUSIC_COVER_PX * 2,
            MUSIC_COVER_PX * 2,
            gdk_pixbuf::InterpType::Bilinear,
        )
        .unwrap_or(square);
    pic.set_paintable(Some(&gdk::Texture::for_pixbuf(&scaled)));
    true
}

struct PreviewPlayer {
    gen: u64,
    track_idx: Option<usize>,
    title: Option<String>,
    path: Option<PathBuf>,
    sock: Option<PathBuf>,
    child: Option<Child>,
    row_btn: Option<SendWeakRef<Button>>,
    volume: f64,
}

impl PreviewPlayer {
    fn new(volume: f64) -> Self {
        Self {
            gen: 0,
            track_idx: None,
            title: None,
            path: None,
            sock: None,
            child: None,
            row_btn: None,
            volume: volume.clamp(0.0, 100.0),
        }
    }

    fn has_track(&self) -> bool {
        self.path.is_some() && self.sock.is_some()
    }

    fn alive(&self) -> bool {
        let Some(child) = self.child.as_ref() else {
            return false;
        };
        let pid = child.id();
        Path::new(&format!("/proc/{pid}")).exists() && self.sock.is_some()
    }

    fn stop(&mut self) {
        self.gen = self.gen.wrapping_add(1);
        if let Some(sock) = &self.sock {
            let _ = mpv_cmd(sock, &serde_json::json!(["quit"]));
        }
        if let Some(mut child) = self.child.take() {
            kill_preview_child(child.id(), &mut child);
        }
        if let Some(sock) = self.sock.take() {
            let _ = std::fs::remove_file(sock);
        }
        if let Some(btn) = self.row_btn.take().and_then(|w| w.upgrade()) {
            set_transport_playing(&btn, false);
        }
        self.track_idx = None;
        self.title = None;
        self.path = None;
    }

    fn load_media(
        &mut self,
        idx: usize,
        title: &str,
        media: &str,
        row_btn: SendWeakRef<Button>,
    ) -> anyhow::Result<()> {
        self.stop();
        self.gen = self.gen.wrapping_add(1);
        let gen = self.gen;

        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let sock = runtime.join(format!("nwall-music-preview-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);

        let mut cmd = Command::new("mpv");
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let vol = self.volume.clamp(0.0, 100.0);
        let http = media.starts_with("http://") || media.starts_with("https://");
        cmd.args([
            "--no-video",
            "--force-window=no",
            "--no-terminal",
            "--ytdl=no",
            "--ao=pulse",
            "--keep-open=yes",
            "--volume-max=100",
            &format!("--volume={vol}"),
            &format!("--input-ipc-server={}", sock.display()),
        ]);
        if http {
            cmd.args([
                &format!("--user-agent={YT_PREVIEW_UA}"),
                "--referrer=https://www.youtube.com/",
                "--network-timeout=30",
            ]);
        }
        cmd.arg(media)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("mpv: {e}"))?;

        let mut ready = false;
        for _ in 0..50 {
            if !Path::new(&format!("/proc/{}", child.id())).exists() {
                break;
            }
            if sock.exists()
                && mpv_cmd(&sock, &serde_json::json!(["get_property", "pause"])).is_ok()
            {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        if !ready {
            kill_preview_child(child.id(), &mut child);
            let _ = std::fs::remove_file(&sock);
            return Err(anyhow::anyhow!("preview player not ready"));
        }

        if gen != self.gen {
            let _ = mpv_cmd(&sock, &serde_json::json!(["quit"]));
            kill_preview_child(child.id(), &mut child);
            let _ = std::fs::remove_file(&sock);
            return Ok(());
        }

        let _ = mpv_cmd(&sock, &serde_json::json!(["set_property", "pause", false]));
        let _ = mpv_cmd(&sock, &serde_json::json!(["set_property", "volume", vol]));
        let _ = mpv_cmd(&sock, &serde_json::json!(["set_property", "mute", false]));

        let mut started = false;
        let wait_n = if http { 150 } else { 80 };
        for _ in 0..wait_n {
            if gen != self.gen {
                let _ = mpv_cmd(&sock, &serde_json::json!(["quit"]));
                kill_preview_child(child.id(), &mut child);
                let _ = std::fs::remove_file(&sock);
                return Ok(());
            }
            if !Path::new(&format!("/proc/{}", child.id())).exists() {
                let _ = std::fs::remove_file(&sock);
                return Err(anyhow::anyhow!("preview player exited before playback"));
            }
            if let Ok(v) = mpv_cmd(&sock, &serde_json::json!(["get_property", "duration"])) {
                if v.get("data").and_then(|d| d.as_f64()).unwrap_or(0.0) > 0.05 {
                    started = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !started {
            let _ = mpv_cmd(&sock, &serde_json::json!(["quit"]));
            kill_preview_child(child.id(), &mut child);
            let _ = std::fs::remove_file(&sock);
            return Err(anyhow::anyhow!("could not start audio (no duration)"));
        }

        self.track_idx = Some(idx);
        self.title = Some(title.to_string());
        self.path = Some(PathBuf::from(media));
        self.sock = Some(sock);
        self.child = Some(child);
        self.row_btn = Some(row_btn);
        Ok(())
    }

    fn set_volume(&mut self, volume: f64) -> anyhow::Result<()> {
        self.volume = volume.clamp(0.0, 100.0);
        let Some(sock) = self.sock.as_ref() else {
            return Ok(());
        };
        mpv_cmd(
            sock,
            &serde_json::json!(["set_property", "volume", self.volume]),
        )?;
        Ok(())
    }

    fn toggle_pause(&self) -> anyhow::Result<bool> {
        let sock = self
            .sock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no preview"))?;
        let paused = self.is_paused().unwrap_or(false);
        let next = !paused;
        mpv_cmd(sock, &serde_json::json!(["set_property", "pause", next]))?;
        Ok(next)
    }

    fn is_paused(&self) -> anyhow::Result<bool> {
        let sock = self
            .sock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no preview"))?;
        let v = mpv_cmd(sock, &serde_json::json!(["get_property", "pause"]))?;
        Ok(v.get("data").and_then(|d| d.as_bool()).unwrap_or(true))
    }

    fn time_pos(&self) -> anyhow::Result<f64> {
        let sock = self
            .sock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no preview"))?;
        let v = mpv_cmd(sock, &serde_json::json!(["get_property", "time-pos"]))?;
        Ok(v.get("data").and_then(|d| d.as_f64()).unwrap_or(0.0))
    }

    fn duration(&self) -> anyhow::Result<f64> {
        let sock = self
            .sock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no preview"))?;
        let v = mpv_cmd(sock, &serde_json::json!(["get_property", "duration"]))?;
        Ok(v.get("data").and_then(|d| d.as_f64()).unwrap_or(0.0))
    }

    fn seek(&self, secs: f64) -> anyhow::Result<()> {
        let sock = self
            .sock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no preview"))?;
        mpv_cmd(
            sock,
            &serde_json::json!(["seek", secs.max(0.0), "absolute"]),
        )?;
        Ok(())
    }
}

impl Drop for PreviewPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn mpv_cmd(sock: &Path, command: &Value) -> anyhow::Result<Value> {
    let mut stream = UnixStream::connect(sock).map_err(|e| anyhow::anyhow!("mpv connect: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(800)))
        .ok();
    stream
        .set_write_timeout(Some(Duration::from_millis(800)))
        .ok();
    let payload = serde_json::json!({ "command": command });
    let mut line = serde_json::to_string(&payload)?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|e| anyhow::anyhow!("mpv write: {e}"))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.contains(&b'\n') {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => break,
            Err(e) => return Err(anyhow::anyhow!("mpv read: {e}")),
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().next().unwrap_or("");
    if line.is_empty() {
        return Ok(Value::Null);
    }
    let v: Value = serde_json::from_str(line).map_err(|e| anyhow::anyhow!("mpv json: {e}"))?;
    if v.get("error").and_then(|e| e.as_str()) != Some("success")
        && v.get("error").is_some()
        && v.get("error") != Some(&Value::Null)
    {
        let err = v
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("mpv error");
        if err != "property unavailable" && err != "success" {
            return Err(anyhow::anyhow!("{err}"));
        }
    }
    Ok(v)
}

fn kill_preview_child(pid: u32, child: &mut Child) {
    if pid != 0 {
        let p = Pid::from_raw(pid as i32);
        let _ = killpg(p, Signal::SIGKILL);
        let _ = kill(p, Signal::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}
