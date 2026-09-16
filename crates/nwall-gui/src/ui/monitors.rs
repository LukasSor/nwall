use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use adw::prelude::*;
use glib::object::SendWeakRef;
use gtk::{
    gdk, glib, Align, Box as GtkBox, ContentFit, Fixed, Label, Orientation, Overlay, Picture,
    ToggleButton,
};
use nwall_ipc::{
    client_request, default_config_path, is_video, Config, OutputStatus, Request, Response,
};

use crate::consts::*;
use crate::ui::preview::bind_thumb;

pub(crate) struct MonitorMock {
    pub(crate) name: String,
    pub(crate) pic: SendWeakRef<Picture>,
    pub(crate) last: Mutex<Option<PathBuf>>,
}

pub(crate) type MonitorMocks = Arc<Vec<MonitorMock>>;

pub(crate) fn make_monitor_mock(
    o: &OutputStatus,
    wallpaper: Option<&Path>,
    scale: f64,
) -> (GtkBox, Picture) {
    let col = GtkBox::new(Orientation::Vertical, 3);
    col.set_halign(Align::Center);
    let sw = ((o.width.max(1) as f64) * scale).round().clamp(28.0, 220.0) as i32;
    let sh = ((o.height.max(1) as f64) * scale).round().clamp(16.0, 90.0) as i32;

    let bezel = GtkBox::new(Orientation::Vertical, 0);
    bezel.add_css_class("monitor-bezel");
    bezel.set_halign(Align::Center);

    let driver = GtkBox::new(Orientation::Vertical, 0);
    driver.set_size_request(sw, sh);
    let pic = Picture::new();
    pic.set_content_fit(ContentFit::Cover);
    pic.set_can_shrink(true);
    pic.set_halign(Align::Fill);
    pic.set_valign(Align::Fill);
    pic.set_hexpand(true);
    pic.set_vexpand(true);
    pic.set_size_request(sw, sh);
    if let Some(wp) = wallpaper {
        bind_monitor_screen(&pic, wp);
    }
    let screen = Overlay::new();
    screen.add_css_class("monitor-screen");
    screen.set_overflow(gtk::Overflow::Hidden);
    screen.set_size_request(sw, sh);
    screen.set_child(Some(&driver));
    screen.add_overlay(&pic);
    screen.set_measure_overlay(&pic, false);
    screen.set_clip_overlay(&pic, true);
    bezel.append(&screen);

    let stand = GtkBox::new(Orientation::Vertical, 0);
    stand.add_css_class("monitor-stand");
    stand.set_halign(Align::Center);
    stand.set_size_request((sw / 4).max(16), 4);

    let name = Label::new(Some(&o.name));
    name.add_css_class("caption");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(14);

    col.append(&bezel);
    col.append(&stand);
    col.append(&name);
    (col, pic)
}

pub(crate) fn wallpaper_for_output(
    name: &str,
    outputs: &[OutputStatus],
    cfg: &Config,
) -> Option<PathBuf> {
    outputs
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| o.wallpaper.clone())
        .or_else(|| cfg.outputs.get(name).cloned())
        .or_else(|| cfg.wallpaper.clone())
}

pub(crate) fn bind_monitor_screen(pic: &Picture, path: &Path) {
    pic.set_filename(Option::<&Path>::None);
    pic.set_paintable(Option::<&gdk::Paintable>::None);
    if is_video(path) {
        bind_thumb(pic, path.to_path_buf(), true);
        return;
    }
    match gdk_pixbuf::Pixbuf::from_file_at_scale(path, MOCK_THUMB_W, MOCK_THUMB_H, true) {
        Ok(pb) => pic.set_paintable(Some(&gdk::Texture::for_pixbuf(&pb))),
        Err(_) => pic.set_filename(Some(path)),
    }
}

pub(crate) fn refresh_monitor_mocks(mocks: &MonitorMocks) -> bool {
    let cfg = Config::load(&default_config_path()).unwrap_or_default();
    let outputs = match client_request(&Request::Status) {
        Ok(Response::Status(s)) => s.outputs,
        _ => Vec::new(),
    };
    let mut changed = false;
    for mock in mocks.iter() {
        let Some(pic) = mock.pic.upgrade() else {
            continue;
        };
        let wp = wallpaper_for_output(&mock.name, &outputs, &cfg);
        let mut last = mock.last.lock().unwrap();
        if last.as_ref() == wp.as_ref() {
            continue;
        }
        *last = wp.clone();
        drop(last);
        changed = true;
        match wp {
            Some(p) => bind_monitor_screen(&pic, &p),
            None => {
                pic.set_filename(Option::<&Path>::None);
                pic.set_paintable(Option::<&gdk::Paintable>::None);
            }
        }
    }
    changed
}

pub(crate) fn fill_monitor_bar(bar: &GtkBox) -> (Rc<RefCell<Vec<String>>>, MonitorMocks) {
    let selected = Rc::new(RefCell::new(Vec::<String>::new()));
    let suppress = Rc::new(Cell::new(false));
    let mut mocks: Vec<MonitorMock> = Vec::new();
    let cfg = Config::load(&default_config_path()).unwrap_or_default();

    let all = ToggleButton::with_label("All");
    all.add_css_class("monitor-all");
    all.set_active(true);
    all.set_valign(Align::Center);
    all.set_vexpand(false);
    all.set_hexpand(false);
    bar.append(&all);

    let mut chips: Vec<(ToggleButton, String)> = Vec::new();
    let mut all_names = Vec::new();
    match client_request(&Request::Status) {
        Ok(Response::Status(s)) if !s.outputs.is_empty() => {
            let mut outputs = s.outputs;
            outputs.sort_by(|a, b| a.x.cmp(&b.x).then(a.y.cmp(&b.y)).then(a.name.cmp(&b.name)));
            let max_h = outputs.iter().map(|o| o.height.max(1)).max().unwrap_or(1);
            let scale = (90.0 / max_h as f64).clamp(0.02, 0.2);
            let min_x = outputs.iter().map(|o| o.x).min().unwrap_or(0);
            let min_y = outputs.iter().map(|o| o.y).min().unwrap_or(0);
            let use_map = outputs.len() > 1 && outputs.iter().any(|o| o.x != min_x || o.y != min_y);
            if use_map {
                let mut xs: Vec<i32> = outputs.iter().map(|o| o.x).collect();
                xs.sort_unstable();
                xs.dedup();
                let mut ys: Vec<i32> = outputs.iter().map(|o| o.y).collect();
                ys.sort_unstable();
                ys.dedup();
                let map = Fixed::new();
                map.set_overflow(gtk::Overflow::Visible);
                let mut placed: Vec<(ToggleButton, f64, f64)> = Vec::new();
                for o in &outputs {
                    let col = xs.iter().position(|&x| x == o.x).unwrap_or(0) as f64;
                    let row = ys.iter().position(|&y| y == o.y).unwrap_or(0) as f64;
                    let px = MONITOR_MOCK_MAP_PAD
                        + (o.x - min_x) as f64 * scale
                        + col * (MONITOR_MOCK_GAP + MONITOR_MOCK_CHROME_X);
                    let py = MONITOR_MOCK_MAP_PAD
                        + (o.y - min_y) as f64 * scale
                        + row * (MONITOR_MOCK_GAP + MONITOR_MOCK_CHROME_Y);
                    let wp = wallpaper_for_output(&o.name, std::slice::from_ref(o), &cfg);
                    let btn = ToggleButton::new();
                    btn.add_css_class("monitor-mock");
                    btn.set_overflow(gtk::Overflow::Visible);
                    btn.set_active(true);
                    let (child, pic) = make_monitor_mock(o, wp.as_deref(), scale);
                    btn.set_child(Some(&child));
                    mocks.push(MonitorMock {
                        name: o.name.clone(),
                        pic: SendWeakRef::from(pic.downgrade()),
                        last: Mutex::new(wp),
                    });
                    all_names.push(o.name.clone());
                    chips.push((btn.clone(), o.name.clone()));
                    map.put(&btn, px, py);
                    placed.push((btn, px, py));
                }
                let mut map_w = MONITOR_MOCK_MAP_PAD;
                let mut map_h = MONITOR_MOCK_MAP_PAD;
                for (btn, px, py) in &placed {
                    let nat_w = btn.measure(gtk::Orientation::Horizontal, -1).1 as f64;
                    let nat_h = btn.measure(gtk::Orientation::Vertical, -1).1 as f64;
                    map_w = map_w.max(px + nat_w + MONITOR_MOCK_MAP_PAD);
                    map_h = map_h.max(py + nat_h + MONITOR_MOCK_MAP_PAD);
                }
                map.set_size_request(map_w.ceil() as i32, map_h.ceil() as i32);
                bar.append(&map);
            } else {
                for o in &outputs {
                    let wp = wallpaper_for_output(&o.name, std::slice::from_ref(o), &cfg);
                    let btn = ToggleButton::new();
                    btn.add_css_class("monitor-mock");
                    btn.set_active(true);
                    let (child, pic) = make_monitor_mock(o, wp.as_deref(), scale);
                    btn.set_child(Some(&child));
                    mocks.push(MonitorMock {
                        name: o.name.clone(),
                        pic: SendWeakRef::from(pic.downgrade()),
                        last: Mutex::new(wp),
                    });
                    all_names.push(o.name.clone());
                    bar.append(&btn);
                    chips.push((btn, o.name.clone()));
                }
            }
        }
        _ => {
            let hint = Label::new(Some("daemon offline"));
            hint.add_css_class("dim-label");
            bar.append(&hint);
        }
    }
    *selected.borrow_mut() = all_names.clone();

    let chips_all = chips.clone();
    let selected_a = Rc::clone(&selected);
    let names_a = all_names.clone();
    let suppress_a = Rc::clone(&suppress);
    all.connect_toggled(move |all| {
        if suppress_a.get() {
            return;
        }
        suppress_a.set(true);
        let on = all.is_active();
        for (btn, _) in &chips_all {
            btn.set_active(on);
        }
        *selected_a.borrow_mut() = if on { names_a.clone() } else { Vec::new() };
        suppress_a.set(false);
    });

    let chip_count = chips.len();
    for (btn, name) in chips {
        let all_c = all.clone();
        let selected_c = Rc::clone(&selected);
        let suppress_c = Rc::clone(&suppress);
        let name_c = name.clone();
        btn.connect_toggled(move |btn| {
            if suppress_c.get() {
                return;
            }
            let mut sel = selected_c.borrow_mut();
            if btn.is_active() {
                if !sel.iter().any(|n| n == &name_c) {
                    sel.push(name_c.clone());
                }
                if sel.len() == chip_count {
                    suppress_c.set(true);
                    all_c.set_active(true);
                    suppress_c.set(false);
                }
            } else {
                sel.retain(|n| n != &name_c);
                suppress_c.set(true);
                all_c.set_active(false);
                suppress_c.set(false);
            }
        });
    }
    (selected, Arc::new(mocks))
}
