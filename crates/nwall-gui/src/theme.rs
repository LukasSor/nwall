
use std::cell::Cell;
use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{
    gdk, glib, EventControllerKey, EventControllerScroll, SpinButton,
};
use nwall_ipc::{default_config_path, Config};

use crate::consts::*;

pub(crate) fn nwall_icon_pixbuf(size: i32) -> gdk_pixbuf::Pixbuf {
    let rgba = nwall_ipc::nwall_icon_rgba(size as u32);
    gdk_pixbuf::Pixbuf::from_bytes(
        &glib::Bytes::from(&rgba),
        gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        size,
        size,
        size * 4,
    )
}

const ICON_SIZES: [i32; 7] = [16, 24, 32, 48, 64, 128, 256];
const DESKTOP_NAME: &str = APP_ID;
const LEGACY_DESKTOP_NAMES: [&str; 2] = ["nwall-gui", "nwall"];

fn desktop_entry() -> String {
    format!(
        "\
[Desktop Entry]
Type=Application
Name=nwall
Comment=Wallpaper picker for niri
Exec=nwall-gui
Icon={APP_ICON_NAME}
Terminal=false
Categories=Graphics;GTK;
StartupWMClass={APP_ID}
"
    )
}

fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
    let mut h = seed;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn icon_stamp_signature(desktop: &str) -> String {
    let rgba = nwall_ipc::nwall_icon_rgba(64);
    let h = fnv1a(desktop.as_bytes(), fnv1a(&rgba, 0xcbf2_9ce4_8422_2325));
    format!(
        "{} {} {} {:016x}\n",
        env!("CARGO_PKG_VERSION"),
        rgba.len(),
        desktop.len(),
        h
    )
}

fn icon_stamp_path() -> std::path::PathBuf {
    glib::user_cache_dir().join("nwall/icon-stamp")
}

fn icon_assets_present(hicolor: &Path, pixmaps: &Path, apps: &Path) -> bool {
    for size in ICON_SIZES {
        let dir = hicolor.join(format!("{size}x{size}/apps"));
        if !dir.join("nwall.png").exists() || !dir.join(format!("{APP_ID}.png")).exists() {
            return false;
        }
    }
    if !pixmaps.join("nwall.png").exists() {
        return false;
    }
    apps.join(format!("{DESKTOP_NAME}.desktop")).exists()
}

pub(crate) fn install_app_icon() {
    let home = glib::home_dir();
    let icons = home.join(".local/share/icons");
    let hicolor = icons.join("hicolor");
    let pixmaps = home.join(".local/share/pixmaps");
    let apps = home.join(".local/share/applications");

    if let Some(display) = gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(&icons);
    }

    let desktop = desktop_entry();
    let signature = icon_stamp_signature(&desktop);
    let stamp = icon_stamp_path();
    let stamped = std::fs::read_to_string(&stamp).is_ok_and(|s| s == signature);
    if stamped && icon_assets_present(&hicolor, &pixmaps, &apps) {
        return;
    }

    for size in ICON_SIZES {
        let dir = hicolor.join(format!("{size}x{size}/apps"));
        let _ = std::fs::create_dir_all(&dir);
        let pb = nwall_icon_pixbuf(size);
        let _ = pb.savev(dir.join("nwall.png"), "png", &[]);
        let _ = pb.savev(dir.join(format!("{APP_ID}.png")), "png", &[]);
    }
    let _ = std::fs::create_dir_all(&pixmaps);
    let _ = nwall_icon_pixbuf(128).savev(pixmaps.join("nwall.png"), "png", &[]);

    let _ = std::fs::create_dir_all(&apps);
    let _ = std::fs::write(apps.join(format!("{DESKTOP_NAME}.desktop")), &desktop);
    for name in LEGACY_DESKTOP_NAMES {
        let _ = std::fs::remove_file(apps.join(format!("{name}.desktop")));
    }

    let _ = Command::new("gtk-update-icon-cache")
        .args(["-f", "-t"])
        .arg(&hicolor)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("update-desktop-database")
        .arg(&apps)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&stamp, &signature);
}

pub(crate) fn apply_theme(_app: &adw::Application, cfg: &Config) {
    let mgr = adw::StyleManager::default();
    match cfg.theme.as_str() {
        "dark" => mgr.set_color_scheme(adw::ColorScheme::ForceDark),
        "light" => mgr.set_color_scheme(adw::ColorScheme::ForceLight),
        _ => mgr.set_color_scheme(adw::ColorScheme::Default),
    }
}

pub(crate) fn load_builtin_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        r#"
        window {
            background: linear-gradient(160deg, alpha(@window_bg_color, 0.96), @window_bg_color);
        }
        .card {
            padding: 8px;
            border-radius: 16px;
            background: alpha(@window_bg_color, 0.42);
            border: 2px solid alpha(white, 0.10);
            outline: none;
            box-shadow: 0 2px 10px alpha(black, 0.18);
        }
        flowboxchild {
            padding: 0;
            outline: none;
            border: none;
            box-shadow: none;
            background: transparent;
        }
        flowboxchild:selected,
        flowboxchild:selected:focus,
        flowboxchild:selected:focus-visible {
            outline: none;
            border: none;
            box-shadow: none;
            background: transparent;
        }
        flowboxchild:selected .card,
        flowboxchild:selected:focus .card,
        flowboxchild:selected:focus-visible .card {
            outline: none;
            border-color: @accent_color;
            background: alpha(@accent_bg_color, 0.22);
            box-shadow: 0 4px 14px alpha(black, 0.22);
        }
        .preview-sidebar {
            min-width: 220px;
            border: none;
            border-left: none;
            box-shadow: none;
        }
        .preview-sidebar .heading,
        .preview-sidebar .dim-label {
            min-width: 0;
        }
        .preview-sidebar .stats-prop-label,
        .preview-sidebar .stats-section-title {
            min-width: max-content;
        }
        .preview-split > separator {
            min-width: 1px;
            min-height: 1px;
            margin: 0;
            padding: 0;
            border: none;
            box-shadow: none;
            background-image: none;
            background-color: alpha(@borders, 0.7);
        }
        .preview-stats-scroll {
            min-height: 120px;
            min-width: 0;
        }
        .preview-stats {
            margin-top: 4px;
            padding: 0 4px;
        }
        .preview-stats-content {
            margin: 0;
            min-width: 0;
        }
        .preview-stats,
        .stats-section,
        .stats-section-rows,
        .stats-table,
        .stats-prop-row {
            min-width: 0;
        }
        .stats-section-title {
            font-size: 0.68em;
            font-weight: 700;
            letter-spacing: 0.08em;
            text-transform: uppercase;
            opacity: 0.78;
            margin: 0;
            padding: 0 2px;
        }
        .stats-prop-row {
            min-height: 0;
            padding: 0;
        }
        .stats-prop-gap {
            min-width: 0;
        }
        .stats-prop-label {
            opacity: 0.58;
            margin: 0;
            padding: 0;
            font-size: 0.92em;
            white-space: nowrap;
        }
        .stats-prop-value {
            font-weight: 400;
            font-size: 1.0em;
            opacity: 0.95;
            margin: 0;
            padding: 0;
            min-width: 0;
            white-space: normal;
        }
        .stats-section-rows {
            min-width: 0;
        }
        .preview-stats-link {
            margin: 0;
            padding: 0;
        }
        .stats-block-body {
            margin: 0;
            padding: 0;
        }
        .stats-tags .preview-tags-cloud,
        .stats-tags.preview-tags-cloud,
        .stats-colors .color-swatches,
        .stats-colors.color-swatches {
            margin-top: 0;
            margin-left: 0;
        }
        .preview-host {
            min-width: 160px;
            min-height: 90px;
            background: alpha(black, 0.4);
            border-radius: 8px;
        }
        .preview-option-row {
            min-height: 0;
            margin-top: 0;
            margin-bottom: 0;
        }
        .source-key-list {
            margin: 8px 0 0 0;
        }
        .source-key-list .source-key-name,
        .source-key-list .source-key-kind {
            min-height: 30px;
            padding-top: 4px;
            padding-bottom: 4px;
        }
        .source-key-list entry.source-api-key,
        .source-key-list .source-api-key-slot {
            min-height: 28px;
            margin: 0;
        }
        .preview-sidebar-btn {
            min-height: 34px;
        }
        .music-preview-btn {
            min-width: 28px;
            min-height: 28px;
            padding: 0;
        }
        .music-preview-ctrl {
            min-width: 28px;
            min-height: 28px;
        }
        .music-track-row {
            padding-top: 2px;
            padding-bottom: 2px;
        }
        .music-cover-wrap {
            min-width: 36px;
            max-width: 36px;
            min-height: 36px;
            max-height: 36px;
            border-radius: 5px;
            background-color: alpha(@window_fg_color, 0.10);
        }
        .music-cover-ph-bg {
            background-color: alpha(@window_fg_color, 0.08);
            border-radius: 5px;
        }
        .music-cover {
            border-radius: 5px;
        }
        .music-cover-ph {
            opacity: 0.55;
        }
        .music-track-title {
            font-weight: 600;
            font-size: 0.92em;
        }
        .preview-tags-cloud {
            min-height: 0;
            min-width: 0;
        }
        .preview-tags-cloud flowboxchild {
            padding: 0;
            margin: 0;
            min-width: 0;
            background: transparent;
            box-shadow: none;
            outline: none;
            border: none;
        }
        .preview-tags-row {
            min-width: 0;
            margin: 0;
            padding: 0;
        }
        box.preview-tag {
            border-radius: 999px;
            padding: 2px 9px;
            border: none;
        }
        box.preview-tag label {
            font-size: 0.82em;
            font-weight: 500;
            color: rgba(255, 255, 255, 0.88);
            white-space: nowrap;
        }
        .preview-loading-layer {
            background-color: rgba(0, 0, 0, 0.38);
        }
        .preview-picture {
            min-width: 0;
            min-height: 0;
        }
        box.color-swatch {
            min-width: 18px;
            min-height: 18px;
            border: none;
            outline: none;
            border-radius: 2px;
            box-shadow: none;
        }
        .thumb-frame {
            min-width: 120px;
            max-width: 120px;
            min-height: 120px;
            max-height: 120px;
            background: alpha(@window_bg_color, 0.32);
            border-radius: 12px;
            border: 1px solid alpha(white, 0.08);
            box-shadow: inset 0 1px 0 alpha(white, 0.06);
        }
        .thumb-placeholder {
            background: alpha(black, 0.55);
        }
        .monitors-collapse {
            padding: 4px 12px;
            border-radius: 999px;
            min-height: 0;
            outline: none;
        }
        .monitors-collapse:hover,
        .monitors-collapse:active,
        .monitors-collapse:focus,
        .monitors-collapse:focus-visible {
            outline: none;
        }
        .monitors-section-sep {
            margin: 6px 20px 2px 20px;
        }
        .monitor-all {
            padding: 4px 14px;
            border-radius: 999px;
        }
        .monitor-all:checked {
            background: @accent_bg_color;
            color: @accent_fg_color;
        }
        .discover-error {
            font-size: 1.45em;
            font-weight: 800;
            color: #e53935;
            padding: 32px 24px;
            line-height: 1.35;
        }
        .discover-loading {
            padding: 48px;
        }
        .discover-status {
            font-size: 1.45em;
            font-weight: 600;
            padding: 32px 24px;
            line-height: 1.35;
        }
        entry.discover-page-entry {
            min-width: 3.2em;
            max-width: 4.6em;
            padding-left: 10px;
            padding-right: 10px;
            padding-top: 4px;
            padding-bottom: 4px;
        }
        .thumb {
            background: transparent;
            border-radius: 12px;
        }
        .monitor-chip {
            padding: 4px 12px;
            border-radius: 999px;
        }
        .monitor-chip:checked {
            background: @accent_bg_color;
            color: @accent_fg_color;
        }
        box.kind-badge, box.kind-badge-image {
            padding: 0;
            border-radius: 999px;
            min-width: 18px;
            min-height: 18px;
            background: alpha(black, 0.58);
            border: 1px solid alpha(white, 0.14);
            box-shadow: 0 1px 2px alpha(black, 0.32);
            color: alpha(white, 0.88);
        }
        box.kind-badge image, box.kind-badge-image image {
            -gtk-icon-size: 11px;
            margin: 0;
            padding: 0;
        }
            /* Border (not outline): GTK outlines ignore radius and clip. */
        .monitor-mock {
            padding: 4px 6px;
            border-radius: 12px;
            background: transparent;
            border: 2px solid transparent;
            outline: none;
            box-shadow: none;
        }
        .monitor-mock:checked {
            background: alpha(@accent_bg_color, 0.22);
            border: 2px solid @accent_color;
            outline: none;
            box-shadow: none;
        }
        .monitor-mock:focus,
        .monitor-mock:focus-visible {
            outline: none;
            box-shadow: none;
        }
        .monitor-bezel {
            background: #1c1c1e;
            border: 1px solid alpha(white, 0.12);
            border-radius: 7px;
            padding: 5px 5px 7px 5px;
        }
        .monitor-screen {
            background: #0b0b0d;
            border-radius: 2px;
            min-width: 24px;
            min-height: 14px;
            overflow: hidden;
        }
        .monitor-stand {
            background: #3a3a3e;
            border-radius: 0 0 3px 3px;
        }
        "#,
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

pub(crate) fn load_css(path: &Path) {
    let provider = gtk::CssProvider::new();
    provider.load_from_path(path);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

pub(crate) fn lock_tile_selection_chrome() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        r#"
        flowboxchild,
        flowboxchild:selected,
        flowboxchild:selected:focus,
        flowboxchild:selected:focus-visible {
            outline: none;
            border: none;
            box-shadow: none;
            background: transparent;
        }
        .card {
            outline: none;
        }
        flowboxchild:selected .card,
        flowboxchild:selected:focus .card,
        flowboxchild:selected:focus-visible .card {
            outline: none;
            border: 2px solid @accent_color;
            background: alpha(@accent_bg_color, 0.22);
            box-shadow: 0 4px 14px alpha(black, 0.22);
        }
        "#,
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 10,
        );
    }
}

pub(crate) fn bind_window_zoom(
    window: &adw::ApplicationWindow,
    zoom: Rc<Cell<f64>>,
    spin: &SpinButton,
) {
    let provider = gtk::CssProvider::new();
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 5,
        );
    }
    let apply = {
        let zoom = Rc::clone(&zoom);
        let provider = provider.clone();
        Rc::new(move || {
            let z = zoom.get();
            let px = (11.0 * z).round();
            let tw = ((THUMB_W as f64) * z).round() as i32;
            let th = ((THUMB_H as f64) * z).round() as i32;
            let badge_min = (18.0 * z).round() as i32;
            let badge_icon = (11.0 * z).round() as i32;
            provider.load_from_string(&format!(
                "window {{ font-size: {px}px; }}\n\
                 .thumb-frame {{ min-width: {tw}px; max-width: {tw}px; min-height: {th}px; max-height: {th}px; }}\n\
                 box.kind-badge, box.kind-badge-image {{ padding: 0; border-radius: 999px; min-width: {badge_min}px; min-height: {badge_min}px; }}\n\
                 box.kind-badge image, box.kind-badge-image image {{ -gtk-icon-size: {badge_icon}px; margin: 0; padding: 0; }}\n"
            ));
        })
    };
    apply();
    let persist = {
        let zoom = Rc::clone(&zoom);
        Rc::new(move || {
            let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
            cfg.gui_zoom = zoom.get();
            let _ = cfg.save(&default_config_path());
        })
    };
    let suppress = Rc::new(Cell::new(false));
    let bump = {
        let zoom = Rc::clone(&zoom);
        let apply = Rc::clone(&apply);
        let persist = Rc::clone(&persist);
        let spin = spin.clone();
        let suppress = Rc::clone(&suppress);
        Rc::new(move |delta: f64| {
            let next = (zoom.get() + delta).clamp(ZOOM_MIN, ZOOM_MAX);
            if (next - zoom.get()).abs() < 0.001 {
                return;
            }
            zoom.set((next * 100.0).round() / 100.0);
            apply();
            suppress.set(true);
            spin.set_value((zoom.get() * 100.0).round());
            suppress.set(false);
            persist();
        })
    };
    {
        let zoom = Rc::clone(&zoom);
        let apply = Rc::clone(&apply);
        let persist = Rc::clone(&persist);
        let suppress_s = Rc::clone(&suppress);
        spin.connect_value_changed(move |s| {
            if suppress_s.get() {
                return;
            }
            let z = (s.value() / 100.0).clamp(ZOOM_MIN, ZOOM_MAX);
            zoom.set((z * 100.0).round() / 100.0);
            apply();
            persist();
        });
    }
    let keys = EventControllerKey::new();
    let bump_k = Rc::clone(&bump);
    let zoom_k = Rc::clone(&zoom);
    let apply_k = Rc::clone(&apply);
    let persist_k = Rc::clone(&persist);
    let spin_k = spin.clone();
    let suppress_k = Rc::clone(&suppress);
    keys.connect_key_pressed(move |_, key, _, mods| {
        if !mods.contains(gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        if key == gdk::Key::plus || key == gdk::Key::equal || key == gdk::Key::KP_Add {
            bump_k(ZOOM_STEP);
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::minus || key == gdk::Key::KP_Subtract {
            bump_k(-ZOOM_STEP);
            return glib::Propagation::Stop;
        }
        if key == gdk::Key::_0 || key == gdk::Key::KP_0 {
            zoom_k.set(1.0);
            apply_k();
            suppress_k.set(true);
            spin_k.set_value(100.0);
            suppress_k.set(false);
            persist_k();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);

    let scroll = EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    let bump_s = Rc::clone(&bump);
    scroll.connect_scroll(move |c, _, dy| {
        if !c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        if dy < 0.0 {
            bump_s(ZOOM_STEP);
        } else if dy > 0.0 {
            bump_s(-ZOOM_STEP);
        }
        glib::Propagation::Stop
    });
    window.add_controller(scroll);
}
