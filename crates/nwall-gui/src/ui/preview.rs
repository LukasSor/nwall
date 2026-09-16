use std::cell::Cell;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adw::prelude::*;
use glib::object::SendWeakRef;
use gtk::{
    gdk, glib, Align, AspectFrame, Box as GtkBox, Button, ContentFit, FlowBoxChild, Label,
    Orientation, Overlay, Paned, Picture, PolicyType, ScrolledWindow, SpinButton, Spinner, Switch,
};
use nwall_catalog as catalog;
use nwall_ipc::{
    client_request, default_config_path, is_video, normalize_preview_width_pct, Config, Request,
    Response,
};

use crate::consts::*;

pub(crate) struct PreviewSession {
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) pid: Arc<AtomicU32>,
    pub(crate) paused: Arc<AtomicBool>,
}

pub(crate) struct LivePreviewStillOut {
    pub(crate) path: PathBuf,
    pub(crate) on_first_frame: Box<dyn FnOnce(&gdk::MemoryTexture) + Send>,
}

#[derive(Clone)]
pub(crate) struct PreviewLoading {
    pub(crate) layer: GtkBox,
    pub(crate) spinner: Spinner,
}

impl PreviewLoading {
    pub(crate) fn set_busy(&self, busy: bool) {
        self.layer.set_visible(busy);
        if busy {
            self.spinner.set_visible(true);
            self.spinner.start();
        } else {
            self.spinner.stop();
            self.spinner.set_visible(false);
        }
    }
}

pub(crate) fn is_http_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

impl Drop for PreviewSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if self.paused.swap(false, Ordering::SeqCst) {
            signal_pg(
                self.pid.load(Ordering::SeqCst),
                nix::sys::signal::Signal::SIGCONT,
            );
        }
        kill_pg(self.pid.load(Ordering::SeqCst));
    }
}

pub(crate) fn stop_live_preview(slot: &Arc<Mutex<Option<PreviewSession>>>) {
    let _killed = slot.lock().unwrap().take();
}

fn signal_pg(pid: u32, sig: nix::sys::signal::Signal) {
    if pid == 0 {
        return;
    }
    let pgid = nix::unistd::Pid::from_raw(pid as i32);
    let _ = nix::sys::signal::killpg(pgid, sig);
    let _ = nix::sys::signal::kill(pgid, sig);
}

pub(crate) fn kill_pg(pid: u32) {
    signal_pg(pid, nix::sys::signal::Signal::SIGKILL);
}

fn set_live_preview_paused(slot: &Arc<Mutex<Option<PreviewSession>>>, paused: bool) {
    let guard = slot.lock().unwrap();
    let Some(session) = guard.as_ref() else {
        return;
    };
    if session.paused.swap(paused, Ordering::SeqCst) == paused {
        return;
    }
    let sig = if paused {
        nix::sys::signal::Signal::SIGSTOP
    } else {
        nix::sys::signal::Signal::SIGCONT
    };
    signal_pg(session.pid.load(Ordering::SeqCst), sig);
}

pub(crate) fn bind_preview_visibility_pause(
    window: &impl IsA<gtk::Widget>,
    live: &Arc<Mutex<Option<PreviewSession>>>,
) {
    const IDLE_AFTER: Duration = Duration::from_millis(900);

    let last_tick = Rc::new(Cell::new(Instant::now()));
    let ticker = Rc::clone(&last_tick);
    window.as_ref().add_tick_callback(move |_, _| {
        ticker.set(Instant::now());
        glib::ControlFlow::Continue
    });

    let live = Arc::clone(live);
    glib::timeout_add_local(Duration::from_millis(400), move || {
        set_live_preview_paused(&live, last_tick.get().elapsed() > IDLE_AFTER);
        glib::ControlFlow::Continue
    });
}

pub(crate) fn configure_child(cmd: &mut Command) {
    cmd.process_group(0);
    unsafe {
        cmd.pre_exec(|| {
            if nix::libc::prctl(nix::libc::PR_SET_PDEATHSIG, nix::libc::SIGKILL) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

pub(crate) fn ffmpeg_cmd() -> Command {
    let mut cmd = Command::new("ffmpeg");
    configure_child(&mut cmd);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-threads",
        "1",
        "-filter_threads",
        "1",
    ]);
    cmd
}

pub(crate) fn kill_child_tree(child: &mut Child) {
    kill_pg(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

pub(crate) fn make_fixed_preview() -> (AspectFrame, Picture, PreviewLoading) {
    let preview_pic = Picture::new();
    preview_pic.set_content_fit(ContentFit::Contain);
    preview_pic.set_can_shrink(true);
    preview_pic.set_halign(Align::Fill);
    preview_pic.set_valign(Align::Fill);
    preview_pic.set_hexpand(true);
    preview_pic.set_vexpand(true);
    preview_pic.add_css_class("thumb");
    preview_pic.add_css_class("preview-picture");
    let host = Overlay::new();
    host.set_hexpand(true);
    host.set_vexpand(true);
    host.set_halign(Align::Fill);
    host.set_valign(Align::Fill);
    host.set_overflow(gtk::Overflow::Hidden);
    host.add_css_class("preview-host");
    let filler = GtkBox::new(Orientation::Vertical, 0);
    filler.set_hexpand(true);
    filler.set_vexpand(true);
    host.set_child(Some(&filler));
    host.add_overlay(&preview_pic);
    host.set_measure_overlay(&preview_pic, false);
    host.set_clip_overlay(&preview_pic, true);

    let layer = GtkBox::new(Orientation::Vertical, 0);
    layer.set_halign(Align::Fill);
    layer.set_valign(Align::Fill);
    layer.set_hexpand(true);
    layer.set_vexpand(true);
    layer.add_css_class("preview-loading-layer");
    layer.set_visible(false);
    let spinner = Spinner::new();
    spinner.set_halign(Align::Center);
    spinner.set_valign(Align::Center);
    spinner.set_hexpand(true);
    spinner.set_vexpand(true);
    spinner.set_size_request(36, 36);
    spinner.set_visible(false);
    spinner.add_css_class("preview-loading");
    layer.append(&spinner);
    host.add_overlay(&layer);
    host.set_clip_overlay(&layer, true);

    let aspect = AspectFrame::new(0.5, 0.0, 16.0 / 9.0, false);
    aspect.set_obey_child(false);
    aspect.set_hexpand(true);
    aspect.set_vexpand(false);
    aspect.set_halign(Align::Fill);
    aspect.set_valign(Align::Start);
    aspect.set_child(Some(&host));
    (aspect, preview_pic, PreviewLoading { layer, spinner })
}

pub(crate) fn new_preview_sidebar() -> GtkBox {
    let preview_box = GtkBox::new(Orientation::Vertical, 6);
    preview_box.set_margin_top(12);
    preview_box.set_margin_bottom(12);
    preview_box.set_margin_start(12);
    preview_box.set_margin_end(12);
    preview_box.set_width_request(SIDEBAR_MIN);
    preview_box.set_hexpand(true);
    preview_box.set_vexpand(true);
    preview_box.set_valign(Align::Fill);
    preview_box.add_css_class("preview-sidebar");
    preview_box
}

pub(crate) fn preview_section_label() -> Label {
    let l = Label::new(Some("Preview"));
    l.set_halign(Align::Center);
    l.set_hexpand(true);
    l
}

pub(crate) fn apply_sidebar_wrap(label: &Label, center: bool) {
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_natural_wrap_mode(gtk::NaturalWrapMode::None);
    label.set_ellipsize(gtk::pango::EllipsizeMode::None);
    label.set_max_width_chars(28);
    label.set_hexpand(true);
    label.set_halign(Align::Fill);
    label.set_xalign(if center { 0.5 } else { 0.0 });
    if center {
        label.set_justify(gtk::Justification::Center);
    }
}

pub(crate) fn bind_wrap_width(label: &Label, inset: i32) {
    let last = Rc::new(Cell::new(0i32));
    label.add_tick_callback(move |l, _| {
        let Some(parent) = l.parent() else {
            return glib::ControlFlow::Continue;
        };
        let width = (parent.width() - inset).max(0);
        if width <= 8 || last.get() == width {
            return glib::ControlFlow::Continue;
        }
        last.set(width);
        let chars = (width / 7).clamp(8, 96);
        if l.max_width_chars() != chars {
            l.set_max_width_chars(chars);
        }
        glib::ControlFlow::Continue
    });
}

pub(crate) fn preview_caption_label(text: &str, heading: bool) -> Label {
    let l = Label::new(Some(text));
    apply_sidebar_wrap(&l, true);
    bind_wrap_width(&l, 0);
    if heading {
        l.add_css_class("heading");
    } else {
        l.add_css_class("dim-label");
    }
    l
}

pub(crate) fn preview_title(text: &str) -> Label {
    let l = Label::new(Some(text));
    apply_sidebar_wrap(&l, true);
    bind_wrap_width(&l, 0);
    l.set_selectable(true);
    l.add_css_class("heading");
    l.set_tooltip_text(Some(text));
    l
}

pub(crate) fn set_preview_title(name: &Label, text: &str) {
    name.set_text(text);
    name.set_tooltip_text(Some(text));
}

pub(crate) fn fill_sidebar_button(btn: &Button) {
    btn.set_halign(Align::Fill);
    btn.set_hexpand(true);
    btn.add_css_class("preview-sidebar-btn");
}

pub(crate) fn preview_sidebar_head() -> GtkBox {
    let head = GtkBox::new(Orientation::Vertical, 6);
    head.set_hexpand(true);
    head.set_vexpand(false);
    head.set_valign(Align::Start);
    head.set_halign(Align::Fill);
    head
}

pub(crate) fn preview_stats_scroll(stats: &impl IsA<gtk::Widget>) -> ScrolledWindow {
    stats.set_hexpand(true);
    stats.set_vexpand(false);
    stats.set_valign(Align::Start);
    stats.set_visible(true);
    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .child(stats)
        .hexpand(true)
        .vexpand(true)
        .propagate_natural_width(false)
        .build();
    // ScrolledWindow: size_request is min-height allocation floor.
    scroll.set_min_content_height(STATS_SCROLL_MIN);
    scroll.set_size_request(-1, STATS_SCROLL_MIN);
    scroll.set_valign(Align::Fill);
    scroll.add_css_class("preview-stats-scroll");
    scroll
}

pub(crate) fn make_preview_split(
    start: &impl IsA<gtk::Widget>,
    end: &impl IsA<gtk::Widget>,
    sidebar_pct: f64,
) -> Paned {
    let split = Paned::new(Orientation::Horizontal);
    split.set_vexpand(true);
    split.set_wide_handle(false);
    split.add_css_class("preview-split");
    split.set_start_child(Some(start));
    split.set_end_child(Some(end));
    split.set_resize_start_child(true);
    split.set_resize_end_child(false);
    split.set_shrink_start_child(true);
    split.set_shrink_end_child(true);
    split.set_position(paned_position_for_pct(1100, sidebar_pct));
    split
}

pub(crate) fn paned_handle_width(split: &Paned) -> i32 {
    let mut child = split.first_child();
    while let Some(c) = child {
        let is_start = split.start_child().is_some_and(|s| s == c);
        let is_end = split.end_child().is_some_and(|e| e == c);
        if !is_start && !is_end {
            let w = c.width();
            if w > 0 {
                return w;
            }
            break;
        }
        child = c.next_sibling();
    }
    if split.is_wide_handle() {
        12
    } else {
        1
    }
}

pub(crate) fn sidebar_px_for_pct(total: i32, handle: i32, pct: f64) -> i32 {
    let inner = (total - handle).max(1);
    let max_side = (inner - 80).max(SIDEBAR_MIN);
    let w = ((inner as f64) * (normalize_preview_width_pct(pct) / 100.0)).round() as i32;
    w.clamp(SIDEBAR_MIN, max_side)
}

pub(crate) fn paned_position_for_pct(total: i32, sidebar_pct: f64) -> i32 {
    let handle = 1;
    let w = sidebar_px_for_pct(total, handle, sidebar_pct);
    (total - handle - w).max(80)
}

pub(crate) fn apply_sidebar_pct(split: &Paned, sidebar_pct: f64) {
    let total = split.width();
    if total <= 0 {
        return;
    }
    let handle = paned_handle_width(split);
    let w = sidebar_px_for_pct(total, handle, sidebar_pct);
    let pos = (total - handle - w).max(80);
    if (split.position() - pos).abs() > 1 {
        split.set_position(pos);
    }
}

pub(crate) fn sidebar_width_from_paned(split: &Paned) -> i32 {
    let total = split.width();
    let pos = split.position();
    let handle = paned_handle_width(split);
    if total > pos + handle {
        (total - pos - handle).max(SIDEBAR_MIN)
    } else {
        SIDEBAR_MIN
    }
}

pub(crate) fn sidebar_pct_from_paned(split: &Paned) -> f64 {
    let total = split.width();
    let handle = paned_handle_width(split);
    let inner = (total - handle).max(1) as f64;
    let side = sidebar_width_from_paned(split) as f64;
    normalize_preview_width_pct((side / inner) * 100.0)
}

pub(crate) fn persist_interface(show_stats: bool, preview_width: f64, gui_zoom: f64) {
    let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
    cfg.show_stats = show_stats;
    cfg.preview_width = normalize_preview_width_pct(preview_width);
    cfg.gui_zoom = gui_zoom;
    let _ = cfg.save(&default_config_path());
}

pub(crate) fn persist_show_monitors(show: bool) {
    let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
    cfg.show_monitors = show;
    let _ = cfg.save(&default_config_path());
}

pub(crate) fn bind_preview_host_size(sidebar: &GtkBox, host: &impl IsA<gtk::Widget>) {
    let host = host.clone().upcast::<gtk::Widget>();
    let last = Rc::new(Cell::new(0i32));
    sidebar.add_tick_callback(move |box_, _clock| {
        let width = box_.width();
        if width <= 0 || last.get() == width {
            return glib::ControlFlow::Continue;
        }
        last.set(width);
        let h = ((width as f64) * 9.0 / 16.0).round().max(1.0) as i32;
        if host.width_request() != -1 || host.height_request() != h {
            host.set_size_request(-1, h);
        }
        queue_stats_tag_relayout(box_);
        glib::ControlFlow::Continue
    });
}

fn queue_stats_tag_relayout(sidebar: &GtkBox) {
    let mut child = sidebar.first_child();
    while let Some(c) = child {
        let next = c.next_sibling();
        if c.has_css_class("preview-stats-scroll") {
            c.queue_allocate();
            c.queue_resize();
        }
        child = next;
    }
}

pub(crate) fn bind_preview_split(
    split: &Paned,
    other: &Paned,
    pct: &Rc<Cell<f64>>,
    suppress: &Rc<Cell<bool>>,
    spin: &SpinButton,
    show_stats: &Rc<Cell<bool>>,
    zoom: &Rc<Cell<f64>>,
    persist_tick: &Rc<Cell<u64>>,
) {
    let other = other.clone();
    let pct = Rc::clone(pct);
    let suppress = Rc::clone(suppress);
    let spin = spin.clone();
    let show_stats = Rc::clone(show_stats);
    let zoom = Rc::clone(zoom);
    let persist_tick = Rc::clone(persist_tick);
    let pct_m = Rc::clone(&pct);
    let suppress_m = Rc::clone(&suppress);
    let pct_map = Rc::clone(&pct);
    let suppress_map = Rc::clone(&suppress);
    let last_total = Rc::new(Cell::new(0i32));
    let last_total_pos = Rc::clone(&last_total);
    split.connect_notify_local(Some("position"), move |p, _| {
        if suppress.get() {
            return;
        }
        let total = p.width();
        if total <= 0 {
            return;
        }
        if last_total_pos.get() != 0 && last_total_pos.get() != total {
            last_total_pos.set(total);
            suppress.set(true);
            apply_sidebar_pct(p, pct.get());
            suppress.set(false);
            return;
        }
        last_total_pos.set(total);
        let next = sidebar_pct_from_paned(p);
        if (pct.get() - next).abs() < 0.5 {
            return;
        }
        pct.set(next);
        suppress.set(true);
        apply_sidebar_pct(&other, next);
        if (spin.value() - next).abs() >= 0.5 {
            spin.set_value(next.round());
        }
        suppress.set(false);
        debounce_persist_interface(&show_stats, &pct, &zoom, &persist_tick);
    });
    split.connect_map(move |p| {
        if suppress_map.get() {
            return;
        }
        suppress_map.set(true);
        apply_sidebar_pct(p, pct_map.get());
        suppress_map.set(false);
    });
    split.connect_notify_local(Some("max-position"), move |p, _| {
        let total = p.width();
        if total <= 0 || last_total.get() == total {
            return;
        }
        last_total.set(total);
        if suppress_m.get() {
            return;
        }
        suppress_m.set(true);
        apply_sidebar_pct(p, pct_m.get());
        suppress_m.set(false);
    });
}

pub(crate) fn debounce_persist_interface(
    show_stats: &Rc<Cell<bool>>,
    preview_width: &Rc<Cell<f64>>,
    gui_zoom: &Rc<Cell<f64>>,
    tick: &Rc<Cell<u64>>,
) {
    let n = tick.get() + 1;
    tick.set(n);
    let show_stats = Rc::clone(show_stats);
    let preview_width = Rc::clone(preview_width);
    let gui_zoom = Rc::clone(gui_zoom);
    let tick = Rc::clone(tick);
    glib::timeout_add_local(Duration::from_millis(400), move || {
        if tick.get() != n {
            return glib::ControlFlow::Break;
        }
        persist_interface(show_stats.get(), preview_width.get(), gui_zoom.get());
        glib::ControlFlow::Break
    });
}

pub(crate) fn ensure_preview_contain(pic: &Picture) {
    pic.set_content_fit(ContentFit::Contain);
}

pub(crate) fn reset_preview_aspect(aspect: &AspectFrame) {
    aspect.set_obey_child(false);
    aspect.set_ratio(16.0 / 9.0);
}

pub(crate) fn fit_preview_still(pic: &Picture, path: &Path) {
    ensure_preview_contain(pic);
    let w = pic.width().max(LIVE_PREV_W as i32);
    let h = pic.height().max(LIVE_PREV_H as i32);
    match gdk_pixbuf::Pixbuf::from_file_at_scale(path, w, h, true) {
        Ok(pb) => pic.set_paintable(Some(&gdk::Texture::for_pixbuf(&pb))),
        Err(_) => {
            ensure_preview_contain(pic);
            pic.set_filename(Some(path));
        }
    }
}

pub(crate) fn fit_tile_still(pic: &Picture, path: &Path) -> bool {
    match gdk_pixbuf::Pixbuf::from_file_at_scale(path, TILE_DECODE_W, TILE_DECODE_H, true) {
        Ok(pb) => {
            pic.set_paintable(Some(&gdk::Texture::for_pixbuf(&pb)));
            true
        }
        Err(_) => false,
    }
}

pub(crate) fn scaled_tile_cache_path(src: &Path) -> PathBuf {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in src.to_string_lossy().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    if let Ok(meta) = std::fs::metadata(src) {
        for b in meta.len().to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        if let Ok(mtime) = meta.modified() {
            if let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH) {
                for b in d.as_secs().to_le_bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
            }
        }
    }
    thumbs_dir().join(format!("{h:016x}-tile.png"))
}

pub(crate) fn purge_scaled_tile(src: &Path) {
    let _ = std::fs::remove_file(scaled_tile_cache_path(src));
}

pub(crate) fn ffmpeg_scale_tile(src: &Path) -> Option<PathBuf> {
    if !file_nonempty(src) {
        return None;
    }
    let dest = scaled_tile_cache_path(src);
    let tmp = dest.with_file_name(format!(
        "{}.part",
        dest.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("tile.png")
    ));
    let _ = std::fs::remove_file(&tmp);
    let mut cmd = ffmpeg_cmd();
    cmd.arg("-y")
        .arg("-i")
        .arg(src)
        .arg("-an")
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(format!(
            "scale={TILE_DECODE_W}:{TILE_DECODE_H}:force_original_aspect_ratio=decrease"
        ))
        .arg(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let ok = wait_child_timeout(&mut child, Duration::from_secs(20)) && file_nonempty(&tmp);
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let _ = std::fs::rename(&tmp, &dest);
    if file_nonempty(&dest) {
        Some(dest)
    } else {
        let _ = std::fs::remove_file(&tmp);
        None
    }
}

pub(crate) fn pixbuf_scale_tile(src: &Path, dest: &Path) -> bool {
    let Ok(pb) = gdk_pixbuf::Pixbuf::from_file_at_scale(src, TILE_DECODE_W, TILE_DECODE_H, true)
    else {
        return false;
    };
    pb.savev(dest, "png", &[]).is_ok() && file_nonempty(dest)
}

pub(crate) fn ensure_scaled_tile(src: &Path) -> Option<PathBuf> {
    if !file_nonempty(src) {
        return None;
    }
    let len = src.metadata().map(|m| m.len()).unwrap_or(0);
    if len > 0 && len <= TILE_RAW_MAX_BYTES {
        if gdk_pixbuf::Pixbuf::from_file_at_scale(src, TILE_DECODE_W, TILE_DECODE_H, true).is_ok() {
            return Some(src.to_path_buf());
        }
        return ffmpeg_scale_tile(src);
    }
    let dest = scaled_tile_cache_path(src);
    if file_nonempty(&dest) {
        if gdk_pixbuf::Pixbuf::from_file_at_scale(&dest, TILE_DECODE_W, TILE_DECODE_H, true).is_ok()
        {
            return Some(dest);
        }
        let _ = std::fs::remove_file(&dest);
    }
    if pixbuf_scale_tile(src, &dest) {
        return Some(dest);
    }
    ffmpeg_scale_tile(src)
}

pub(crate) fn apply_tile_image(pic: &Picture, src: &Path) {
    if let Some(path) = ensure_scaled_tile(src) {
        let _ = fit_tile_still(pic, &path);
    }
}

pub(crate) fn daemon_playing_path(path: &Path) -> bool {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match client_request(&Request::Status) {
        Ok(Response::Status(st)) if st.fps > 0 => st.outputs.iter().any(|o| {
            o.kind == "video"
                && o.wallpaper
                    .as_ref()
                    .is_some_and(|p| p == &canon || p == path)
        }),
        _ => false,
    }
}

pub(crate) fn show_preview(
    path: &Path,
    pic: &Picture,
    name: &Label,
    meta: &Label,
    apply: &Button,
    delete: &Button,
    live: &Arc<Mutex<Option<PreviewSession>>>,
    gen: &Arc<AtomicU64>,
    animate: bool,
    loading: &PreviewLoading,
    tile: Option<&FlowBoxChild>,
) {
    stop_live_preview(live);
    let next = gen.fetch_add(1, Ordering::SeqCst) + 1;
    set_preview_title(name, &catalog::library_caption(path));
    pic.set_paintable(Option::<&gdk::Paintable>::None);
    ensure_preview_contain(pic);
    loading.set_busy(false);
    if is_video(path) {
        ensure_preview_contain(pic);
        bind_thumb(pic, path.to_path_buf(), true);
        if animate && !daemon_playing_path(path) {
            meta.set_text("Playing preview video");
            arm_preview_loading(loading, gen, next);
            let still_out = tile.and_then(|child| {
                use crate::ui::discover::thumbs::{
                    refresh_remote_tile_still_if_missing, refresh_remote_tile_texture_if_missing,
                    remote_tile_needs_still,
                };
                if !remote_tile_needs_still(child) {
                    return None;
                }
                let dest = thumb_cache_path(path);
                if file_nonempty(&dest) {
                    refresh_remote_tile_still_if_missing(child, &dest);
                    if !remote_tile_needs_still(child) {
                        return None;
                    }
                }
                let tile_w = SendWeakRef::from(child.downgrade());
                Some(LivePreviewStillOut {
                    path: dest,
                    on_first_frame: Box::new(move |tex| {
                        if let Some(c) = tile_w.upgrade() {
                            refresh_remote_tile_texture_if_missing(&c, tex);
                        }
                    }),
                })
            });
            let _ = start_live_preview(
                path.to_string_lossy().into_owned(),
                pic,
                live,
                gen,
                next,
                loading,
                still_out,
            );
        } else if animate {
            meta.set_text("Already on desktop");
        } else {
            meta.set_text("Video — still (FPS 0)");
        }
    } else {
        meta.set_text("Showing preview image");
        fit_preview_still(pic, path);
    }
    apply.set_sensitive(true);
    delete.set_sensitive(path.is_file());
}

pub(crate) fn refresh_bg_music_ui(
    path: Option<&Path>,
    file_btn: &Button,
    archive_btn: &Button,
    yt_btn: &Button,
    remove_btn: &Button,
    mute: &Switch,
    vol: &SpinButton,
    mute_row: &GtkBox,
    vol_row: &GtkBox,
    name_lbl: &Label,
    suppress: &Rc<Cell<bool>>,
) {
    let Some(path) = path.filter(|p| p.is_file()) else {
        file_btn.set_sensitive(false);
        archive_btn.set_sensitive(false);
        yt_btn.set_sensitive(false);
        remove_btn.set_visible(false);
        mute_row.set_visible(false);
        vol_row.set_visible(false);
        name_lbl.set_visible(false);
        return;
    };
    file_btn.set_sensitive(true);
    archive_btn.set_sensitive(true);
    yt_btn.set_sensitive(true);
    let meta = catalog::load_path_meta(path);
    let music = meta
        .as_ref()
        .and_then(|s| s.bg_music.as_ref())
        .filter(|p| p.is_file());
    if let Some(m) = music {
        remove_btn.set_visible(true);
        mute_row.set_visible(true);
        vol_row.set_visible(true);
        name_lbl.set_visible(true);
        let fname = m.file_name().and_then(|s| s.to_str()).unwrap_or("audio");
        name_lbl.set_text(fname);
        name_lbl.set_tooltip_text(Some(&m.display().to_string()));
        suppress.set(true);
        mute.set_active(meta.as_ref().map(|s| s.bg_music_muted()).unwrap_or(false));
        vol.set_value(
            (meta
                .as_ref()
                .map(|s| s.bg_music_volume_or_default())
                .unwrap_or(0.5)
                * 100.0) as f64,
        );
        suppress.set(false);
    } else {
        remove_btn.set_visible(false);
        mute_row.set_visible(false);
        vol_row.set_visible(false);
        name_lbl.set_visible(false);
        name_lbl.set_tooltip_text(None);
    }
}

pub(crate) fn hide_preview_loading(layer: &SendWeakRef<GtkBox>, spinner: &SendWeakRef<Spinner>) {
    let Some(layer) = layer.upgrade() else {
        return;
    };
    let Some(spinner) = spinner.upgrade() else {
        return;
    };
    PreviewLoading { layer, spinner }.set_busy(false);
}

pub(crate) fn arm_preview_loading(loading: &PreviewLoading, gen: &Arc<AtomicU64>, my: u64) {
    loading.set_busy(true);
    let loading = loading.clone();
    let gen = Arc::clone(gen);
    glib::timeout_add_local_once(PREVIEW_LOAD_TIMEOUT, move || {
        if gen.load(Ordering::SeqCst) != my {
            return;
        }
        loading.set_busy(false);
    });
}

pub(crate) fn start_live_preview(
    input: String,
    pic: &Picture,
    live: &Arc<Mutex<Option<PreviewSession>>>,
    gen: &Arc<AtomicU64>,
    my_gen: u64,
    loading: &PreviewLoading,
    mut still_out: Option<LivePreviewStillOut>,
) -> Arc<AtomicU64> {
    stop_live_preview(live);
    let stop = Arc::new(AtomicBool::new(false));
    let pid = Arc::new(AtomicU32::new(0));
    let frames = Arc::new(AtomicU64::new(0));
    *live.lock().unwrap() = Some(PreviewSession {
        stop: Arc::clone(&stop),
        pid: Arc::clone(&pid),
        paused: Arc::new(AtomicBool::new(false)),
    });
    let weak = SendWeakRef::from(pic.downgrade());
    let gen = Arc::clone(gen);
    let frames_t = Arc::clone(&frames);
    let layer_w = SendWeakRef::from(loading.layer.downgrade());
    let spinner_w = SendWeakRef::from(loading.spinner.downgrade());
    std::thread::spawn(move || {
        let stale = || stop.load(Ordering::SeqCst) || gen.load(Ordering::SeqCst) != my_gen;
        let hide_loading = {
            let layer_w = layer_w.clone();
            let spinner_w = spinner_w.clone();
            let gen = Arc::clone(&gen);
            move || {
                let layer_w = layer_w.clone();
                let spinner_w = spinner_w.clone();
                let gen = Arc::clone(&gen);
                glib::idle_add_once(move || {
                    if gen.load(Ordering::SeqCst) != my_gen {
                        return;
                    }
                    hide_preview_loading(&layer_w, &spinner_w);
                });
            }
        };
        if stale() {
            return;
        }
        let input = if is_http_url(&input) {
            catalog::resolve_remote_url(&input).unwrap_or(input)
        } else {
            input
        };
        if stale() {
            hide_loading();
            return;
        }
        let vf = format!(
            "fps={LIVE_PREV_FPS},scale={LIVE_PREV_W}:{LIVE_PREV_H}:force_original_aspect_ratio=decrease:flags=fast_bilinear,pad={LIVE_PREV_W}:{LIVE_PREV_H}:(ow-iw)/2:(oh-ih)/2,format=rgba"
        );
        let mut cmd = ffmpeg_cmd();
        if is_http_url(&input) {
            cmd.arg("-user_agent").arg(catalog::UA);
        }
        let mut child = match cmd
            .arg("-stream_loop")
            .arg("-1")
            .arg("-i")
            .arg(&input)
            .arg("-an")
            .arg("-vf")
            .arg(&vf)
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgba")
            .arg("pipe:1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                hide_loading();
                return;
            }
        };
        pid.store(child.id(), Ordering::SeqCst);
        if stale() {
            kill_child_tree(&mut child);
            pid.store(0, Ordering::SeqCst);
            return;
        }
        let mut stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                kill_child_tree(&mut child);
                pid.store(0, Ordering::SeqCst);
                hide_loading();
                return;
            }
        };
        let frame_bytes = (LIVE_PREV_W * LIVE_PREV_H * 4) as usize;
        let mut buf = vec![0u8; frame_bytes];
        let inflight = Arc::new(AtomicBool::new(false));
        let interval = Duration::from_millis(1000 / u64::from(LIVE_PREV_FPS.max(1)));
        let mut next_due = Instant::now();
        let mut pending_tile_cb: Option<Box<dyn FnOnce(&gdk::MemoryTexture) + Send>> = None;
        while !stale() {
            if stdout.read_exact(&mut buf).is_err() {
                break;
            }
            if let Some(sink) = still_out.take() {
                if !file_nonempty(&sink.path) {
                    let _ = save_rgba_still_jpeg(&buf, LIVE_PREV_W, LIVE_PREV_H, &sink.path);
                }
                pending_tile_cb = Some(sink.on_first_frame);
            }
            if !inflight.swap(true, Ordering::Relaxed) {
                let bytes = glib::Bytes::from(&buf);
                let weak = weak.clone();
                let gen = Arc::clone(&gen);
                let inflight = Arc::clone(&inflight);
                let frames = Arc::clone(&frames_t);
                let layer_w = layer_w.clone();
                let spinner_w = spinner_w.clone();
                let tile_cb = pending_tile_cb.take();
                glib::idle_add_once(move || {
                    inflight.store(false, Ordering::Relaxed);
                    let tex = gdk::MemoryTexture::new(
                        LIVE_PREV_W as i32,
                        LIVE_PREV_H as i32,
                        gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        (LIVE_PREV_W * 4) as usize,
                    );
                    if let Some(cb) = tile_cb {
                        cb(&tex);
                    }
                    if gen.load(Ordering::SeqCst) != my_gen {
                        return;
                    }
                    let Some(p) = weak.upgrade() else {
                        return;
                    };
                    ensure_preview_contain(&p);
                    p.set_paintable(Some(&tex));
                    if frames.fetch_add(1, Ordering::Relaxed) == 0 {
                        hide_preview_loading(&layer_w, &spinner_w);
                    }
                });
            }
            if stale() {
                break;
            }
            let now = Instant::now();
            if next_due > now {
                let deadline = next_due;
                while Instant::now() < deadline {
                    if stale() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                next_due += interval;
            } else {
                next_due = now + interval;
            }
        }
        kill_child_tree(&mut child);
        pid.store(0, Ordering::SeqCst);
        if frames_t.load(Ordering::Relaxed) == 0 {
            hide_loading();
        }
    });
    frames
}

pub(crate) fn save_rgba_still_jpeg(rgba: &[u8], w: u32, h: u32, dest: &Path) -> bool {
    let expected = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if rgba.len() < expected || w == 0 || h == 0 {
        return false;
    }
    let tmp = dest.with_extension("part.jpg");
    let _ = std::fs::remove_file(&tmp);
    let px = (w as usize).saturating_mul(h as usize);
    let mut rgb = vec![0u8; px.saturating_mul(3)];
    for i in 0..px {
        let s = i * 4;
        let d = i * 3;
        rgb[d] = rgba[s];
        rgb[d + 1] = rgba[s + 1];
        rgb[d + 2] = rgba[s + 2];
    }
    let pb = gdk_pixbuf::Pixbuf::from_bytes(
        &glib::Bytes::from(&rgb[..]),
        gdk_pixbuf::Colorspace::Rgb,
        false,
        8,
        w as i32,
        h as i32,
        (w * 3) as i32,
    );
    if pb.savev(&tmp, "jpeg", &[("quality", "82")]).is_err() || !file_nonempty(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    match std::fs::rename(&tmp, dest) {
        Ok(()) => file_nonempty(dest),
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            false
        }
    }
}

pub(crate) fn thumbs_dir() -> PathBuf {
    let mut dir = if let Some(c) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(c)
    } else {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".cache")
    };
    dir.push("nwall");
    dir.push("thumbs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub(crate) fn file_nonempty(path: &Path) -> bool {
    path.metadata().map(|m| m.len() > 0).unwrap_or(false)
}

pub(crate) fn wait_child_timeout(child: &mut Child, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(40));
            }
            _ => {
                kill_child_tree(child);
                return false;
            }
        }
    }
}

pub(crate) fn thumb_cache_path(src: &Path) -> PathBuf {
    let meta = std::fs::metadata(src).ok();
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let len = meta.map(|m| m.len()).unwrap_or(0);
    let mut h: u64 = 0xcbf29ce484222325;
    for b in src.to_string_lossy().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    for b in mtime.to_le_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    for b in len.to_le_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    thumbs_dir().join(format!("{h:016x}.jpg"))
}

pub(crate) fn extract_thumb(src: &Path, dest: &Path) -> bool {
    const THUMB_TIMEOUT: Duration = Duration::from_secs(8);
    let run = |ss: &str| {
        let mut cmd = ffmpeg_cmd();
        let mut child = match cmd
            .args([
                "-y",
                "-ss",
                ss,
                "-i",
                &src.to_string_lossy(),
                "-an",
                "-frames:v",
                "1",
                "-vf",
                "scale=480:-2:flags=fast_bilinear:force_original_aspect_ratio=decrease",
                "-q:v",
                "5",
                &dest.to_string_lossy(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        wait_child_timeout(&mut child, THUMB_TIMEOUT) && file_nonempty(dest)
    };
    run("1") || run("0")
}

pub(crate) struct ThumbJob {
    src: PathBuf,
    dest: PathBuf,
    pic: SendWeakRef<Picture>,
    video: bool,
}

pub(crate) fn thumb_tx() -> std::sync::mpsc::Sender<ThumbJob> {
    use std::sync::{mpsc, Mutex, OnceLock};
    static TX: OnceLock<mpsc::Sender<ThumbJob>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<ThumbJob>();
        let rx = std::sync::Arc::new(Mutex::new(rx));
        for i in 0..2 {
            let rx = std::sync::Arc::clone(&rx);
            let _ = std::thread::Builder::new()
                .name(format!("nwall-thumb-{i}"))
                .spawn(move || loop {
                    let job = {
                        let Ok(guard) = rx.lock() else {
                            break;
                        };
                        match guard.recv() {
                            Ok(j) => j,
                            Err(_) => break,
                        }
                    };
                    let show = if file_nonempty(&job.dest) {
                        Some(job.dest.clone())
                    } else if job.video {
                        extract_thumb(&job.src, &job.dest).then_some(job.dest.clone())
                    } else {
                        ensure_scaled_tile(&job.src)
                    };
                    let pic = job.pic;
                    glib::idle_add_once(move || {
                        if let (Some(p), Some(path)) = (pic.upgrade(), show) {
                            let _ = fit_tile_still(&p, &path);
                        }
                    });
                });
        }
        tx
    })
    .clone()
}

pub(crate) fn bind_thumb(pic: &Picture, path: PathBuf, video: bool) {
    if !video {
        let dest = scaled_tile_cache_path(&path);
        if file_nonempty(&dest) && fit_tile_still(pic, &dest) {
            return;
        }
        let _ = thumb_tx().send(ThumbJob {
            src: path,
            dest,
            pic: SendWeakRef::from(pic.downgrade()),
            video: false,
        });
        return;
    }
    let dest = thumb_cache_path(&path);
    if dest.exists() {
        apply_tile_image(pic, &dest);
        return;
    }
    let _ = thumb_tx().send(ThumbJob {
        src: path,
        dest,
        pic: SendWeakRef::from(pic.downgrade()),
        video: true,
    });
}
