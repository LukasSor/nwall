
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use glib::object::SendWeakRef;
use gtk::{
    gdk, glib, Align, Box as GtkBox, ContentFit, Fixed, FlowBox, FlowBoxChild, Frame, Label,
    Orientation, Overlay, Picture,
};
use nwall_ipc::{config_dir, is_image, is_video};

use crate::consts::*;
use crate::ui::preview::{
    apply_tile_image, ensure_scaled_tile, ffmpeg_cmd, file_nonempty, fit_tile_still, kill_child_tree,
    purge_scaled_tile, scaled_tile_cache_path, thumbs_dir, wait_child_timeout,
};
use nwall_catalog as catalog;

pub(crate) fn remote_thumb_gen() -> &'static AtomicU64 {
    static GEN: std::sync::OnceLock<AtomicU64> = std::sync::OnceLock::new();
    GEN.get_or_init(|| AtomicU64::new(1))
}

pub(crate) fn bump_remote_thumb_gen() -> u64 {
    remote_thumb_gen().fetch_add(1, Ordering::Relaxed) + 1
}

pub(crate) fn make_fixed_thumb() -> (Overlay, Picture) {
    let pic = Picture::new();
    pic.set_content_fit(ContentFit::Contain);
    pic.set_can_shrink(true);
    pic.set_halign(Align::Fill);
    pic.set_valign(Align::Fill);
    pic.add_css_class("thumb");
    let driver = GtkBox::new(Orientation::Vertical, 0);
    driver.set_size_request(THUMB_W, THUMB_H);
    let host = Overlay::new();
    host.set_size_request(THUMB_W, THUMB_H);
    host.set_hexpand(false);
    host.set_vexpand(false);
    host.set_overflow(gtk::Overflow::Hidden);
    host.add_css_class("thumb-frame");
    host.set_child(Some(&driver));
    host.add_overlay(&pic);
    host.set_measure_overlay(&pic, false);
    host.set_clip_overlay(&pic, true);
    (host, pic)
}

pub(crate) fn pack_tile(thumb: &Overlay, title: &str) -> FlowBoxChild {
    let v = GtkBox::new(Orientation::Vertical, 4);
    v.set_size_request(TILE_W, THUMB_H + 28);
    v.set_hexpand(false);
    v.set_vexpand(false);
    v.set_halign(Align::Center);
    v.set_valign(Align::Start);
    let name = Label::new(Some(title));
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(16);
    name.set_single_line_mode(true);
    name.set_width_chars(16);
    v.append(thumb);
    v.append(&name);
    let frame = Frame::new(None);
    frame.add_css_class("card");
    frame.set_size_request(TILE_W, THUMB_H + 36);
    frame.set_hexpand(false);
    frame.set_vexpand(false);
    frame.set_valign(Align::Start);
    frame.set_halign(Align::Center);
    frame.set_child(Some(&v));
    let child = FlowBoxChild::new();
    child.set_valign(Align::Start);
    child.set_halign(Align::Start);
    child.set_hexpand(false);
    child.set_vexpand(false);
    child.set_size_request(TILE_W, THUMB_H + 40);
    child.set_child(Some(&frame));
    unsafe {
        child.set_data("nwall-caption", name);
    }
    child
}

pub(crate) fn apply_video_tile_still(pic: &Picture, host: Option<&Overlay>, still: &Path) {
    if !file_nonempty(still) {
        return;
    }
    apply_tile_image(pic, still);
    if let Some(h) = host {
        h.remove_css_class("thumb-placeholder");
    }
}

pub(crate) fn apply_video_tile_texture(
    pic: &Picture,
    host: Option<&Overlay>,
    tex: &gdk::MemoryTexture,
) {
    pic.set_paintable(Some(tex));
    if let Some(h) = host {
        h.remove_css_class("thumb-placeholder");
    }
}

pub(crate) fn remote_tile_needs_still(child: &FlowBoxChild) -> bool {
    let Some((pic, host)) = remote_tile_widgets(child) else {
        return false;
    };
    host.has_css_class("thumb-placeholder") || pic.paintable().is_none()
}

pub(crate) fn refresh_remote_tile_still(child: &FlowBoxChild, still: &Path) {
    if !file_nonempty(still) {
        return;
    }
    let (pic, host) = match remote_tile_widgets(child) {
        Some(pair) => pair,
        None => return,
    };
    apply_video_tile_still(&pic, Some(&host), still);
}

pub(crate) fn refresh_remote_tile_still_if_missing(child: &FlowBoxChild, still: &Path) {
    if !remote_tile_needs_still(child) {
        return;
    }
    refresh_remote_tile_still(child, still);
}

pub(crate) fn refresh_remote_tile_texture_if_missing(
    child: &FlowBoxChild,
    tex: &gdk::MemoryTexture,
) {
    if !remote_tile_needs_still(child) {
        return;
    }
    let (pic, host) = match remote_tile_widgets(child) {
        Some(pair) => pair,
        None => return,
    };
    apply_video_tile_texture(&pic, Some(&host), tex);
}

pub(crate) fn remote_tile_widgets(child: &FlowBoxChild) -> Option<(Picture, Overlay)> {
    if let (Some(pic), Some(host)) = (remote_tile_pic(child), remote_tile_host(child)) {
        return Some((pic, host));
    }
    walk_remote_tile_widgets(child)
}

fn walk_remote_tile_widgets(child: &FlowBoxChild) -> Option<(Picture, Overlay)> {
    let frame = child.child()?.downcast::<Frame>().ok()?;
    let vbox = frame.child()?.downcast::<GtkBox>().ok()?;
    let host = vbox.first_child()?.downcast::<Overlay>().ok()?;
    let pic = overlay_tile_picture(&host)?;
    Some((pic, host))
}

fn overlay_tile_picture(host: &Overlay) -> Option<Picture> {
    // GTK4 Overlay keeps the main child + overlays in the widget child list.
    let mut w = host.first_child();
    while let Some(cur) = w {
        if let Ok(pic) = cur.clone().downcast::<Picture>() {
            return Some(pic);
        }
        w = cur.next_sibling();
    }
    None
}

pub(crate) fn remote_tile_pic(child: &FlowBoxChild) -> Option<Picture> {
    unsafe {
        child
            .data::<Picture>("nwall-tile-pic")
            .map(|p| p.as_ref().clone())
    }
    .or_else(|| walk_remote_tile_widgets(child).map(|(p, _)| p))
}

pub(crate) fn remote_tile_host(child: &FlowBoxChild) -> Option<Overlay> {
    unsafe {
        child
            .data::<Overlay>("nwall-tile-host")
            .map(|p| p.as_ref().clone())
    }
    .or_else(|| walk_remote_tile_widgets(child).map(|(_, h)| h))
}

pub(crate) fn bind_remote_image_thumb_prio(pic: &Picture, url: &str, priority: bool) {
    let dest = catalog::cached_path(url, "thumb");
    if file_nonempty(&dest) {
        let len = dest.metadata().map(|m| m.len()).unwrap_or(0);
        if len > 0 && len <= TILE_RAW_MAX_BYTES {
            if fit_tile_still(pic, &dest) {
                return;
            }
        } else {
            let scaled = scaled_tile_cache_path(&dest);
            if file_nonempty(&scaled) && fit_tile_still(pic, &scaled) {
                return;
            }
        }
    }
    let gen = remote_thumb_gen().load(Ordering::Relaxed);
    let _ = remote_thumb_tx().send(RemoteThumbJob {
        url: url.to_string(),
        dest,
        pic: SendWeakRef::from(pic.downgrade()),
        priority,
        gen,
    });
}

pub(crate) struct RemoteThumbJob {
    url: String,
    dest: PathBuf,
    pic: SendWeakRef<Picture>,
    priority: bool,
    gen: u64,
}

pub(crate) fn remote_thumb_tx() -> std::sync::mpsc::Sender<RemoteThumbJob> {
    use std::collections::VecDeque;
    use std::sync::{mpsc, Condvar, Mutex, OnceLock};
    static TX: OnceLock<mpsc::Sender<RemoteThumbJob>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<RemoteThumbJob>();
        let queue: std::sync::Arc<(Mutex<VecDeque<RemoteThumbJob>>, Condvar)> =
            std::sync::Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let q_push = std::sync::Arc::clone(&queue);
        let _ = std::thread::Builder::new()
            .name("nwall-rthumb-ingress".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let live = remote_thumb_gen().load(Ordering::Relaxed);
                    if job.gen != live {
                        continue;
                    }
                    let (mu, cv) = &*q_push;
                    let mut q = mu.lock().unwrap();
                    q.retain(|j| j.gen == live);
                    while q.len() >= REMOTE_THUMB_QUEUE_MAX {
                        if let Some(pos) = q.iter().rposition(|j| !j.priority) {
                            q.remove(pos);
                        } else {
                            q.pop_back();
                        }
                    }
                    if job.priority {
                        q.push_front(job);
                    } else {
                        q.push_back(job);
                    }
                    cv.notify_one();
                }
            });
        let workers = catalog::REMOTE_THUMB_HTTP_MAX.max(2);
        for i in 0..workers {
            let q_take = std::sync::Arc::clone(&queue);
            let _ = std::thread::Builder::new()
                .name(format!("nwall-rthumb-{i}"))
                .spawn(move || loop {
                    let job = {
                        let (mu, cv) = &*q_take;
                        let mut q = mu.lock().unwrap();
                        loop {
                            let live = remote_thumb_gen().load(Ordering::Relaxed);
                            q.retain(|j| j.gen == live);
                            if let Some(job) = q.pop_front() {
                                break job;
                            }
                            q = cv.wait(q).unwrap();
                        }
                    };
                    if job.gen != remote_thumb_gen().load(Ordering::Relaxed) {
                        continue;
                    }
                    let scaled = materialize_remote_tile(&job.url, &job.dest, job.gen);
                    if job.gen != remote_thumb_gen().load(Ordering::Relaxed) {
                        continue;
                    }
                    let pic = job.pic;
                    let gen = job.gen;
                    glib::idle_add_once(move || {
                        if gen != remote_thumb_gen().load(Ordering::Relaxed) {
                            return;
                        }
                        if let (Some(p), Some(path)) = (pic.upgrade(), scaled) {
                            let _ = fit_tile_still(&p, &path);
                        }
                    });
                });
        }
        tx
    })
    .clone()
}

pub(crate) fn materialize_remote_tile(url: &str, dest: &Path, gen: u64) -> Option<PathBuf> {
    const ATTEMPTS: u32 = 3;
    for attempt in 0..ATTEMPTS {
        if gen != remote_thumb_gen().load(Ordering::Relaxed) {
            return None;
        }
        if attempt > 0 {
            let _ = std::fs::remove_file(dest);
            purge_scaled_tile(dest);
            std::thread::sleep(Duration::from_millis(120 * u64::from(attempt)));
        }
        let path = if attempt == 0 && file_nonempty(dest) {
            dest.to_path_buf()
        } else if attempt == 0 {
            match catalog::download_thumb(url) {
                Ok(p) => p,
                Err(_) => continue,
            }
        } else {
            match catalog::redownload_thumb(url) {
                Ok(p) => p,
                Err(_) => continue,
            }
        };
        if let Some(scaled) = ensure_scaled_tile(&path) {
            return Some(scaled);
        }
        let _ = std::fs::remove_file(&path);
        purge_scaled_tile(&path);
    }
    None
}

pub(crate) fn remote_still_path(url: &str) -> PathBuf {
    thumbs_dir().join(format!("{}.jpg", catalog::hash_url(url)))
}

pub(crate) fn extract_remote_still(url: &str, dest: &Path) -> bool {
    if file_nonempty(dest) {
        return true;
    }
    let tmp = dest.with_file_name(format!(
        "{}.part.jpg",
        dest.file_stem().and_then(|s| s.to_str()).unwrap_or("still")
    ));
    let _ = std::fs::remove_file(&tmp);
    let ok = ffmpeg_http_still(url, &tmp, true) || ffmpeg_http_still(url, &tmp, false);
    if ok && file_nonempty(&tmp) {
        let _ = std::fs::rename(&tmp, dest);
        file_nonempty(dest)
    } else {
        let _ = std::fs::remove_file(&tmp);
        false
    }
}

pub(crate) fn bind_remote_video_still(child: &FlowBoxChild, url: &str, priority: bool) {
    let dest = remote_still_path(url);
    if file_nonempty(&dest) {
        refresh_remote_tile_still(child, &dest);
        return;
    }
    let gen = remote_thumb_gen().load(Ordering::Relaxed);
    let _ = remote_still_tx().send(RemoteStillJob {
        url: url.to_string(),
        dest,
        child: SendWeakRef::from(child.downgrade()),
        priority,
        gen,
    });
}

struct RemoteStillJob {
    url: String,
    dest: PathBuf,
    child: SendWeakRef<FlowBoxChild>,
    priority: bool,
    gen: u64,
}

fn remote_still_tx() -> std::sync::mpsc::Sender<RemoteStillJob> {
    use std::collections::VecDeque;
    use std::sync::{mpsc, Condvar, Mutex, OnceLock};
    static TX: OnceLock<mpsc::Sender<RemoteStillJob>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<RemoteStillJob>();
        let queue: Arc<(Mutex<VecDeque<RemoteStillJob>>, Condvar)> =
            Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let q_push = Arc::clone(&queue);
        let _ = std::thread::Builder::new()
            .name("nwall-rstill-ingress".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let live = remote_thumb_gen().load(Ordering::Relaxed);
                    if job.gen != live {
                        continue;
                    }
                    let (mu, cv) = &*q_push;
                    let mut q = mu.lock().unwrap();
                    q.retain(|j| j.gen == live);
                    while q.len() >= REMOTE_STILL_QUEUE_MAX {
                        if let Some(pos) = q.iter().rposition(|j| !j.priority) {
                            q.remove(pos);
                        } else {
                            q.pop_back();
                        }
                    }
                    if job.priority {
                        q.push_front(job);
                    } else {
                        q.push_back(job);
                    }
                    cv.notify_one();
                }
            });
        for i in 0..REMOTE_STILL_WORKERS {
            let q_take = Arc::clone(&queue);
            let _ = std::thread::Builder::new()
                .name(format!("nwall-rstill-{i}"))
                .spawn(move || loop {
                    let job = {
                        let (mu, cv) = &*q_take;
                        let mut q = mu.lock().unwrap();
                        loop {
                            let live = remote_thumb_gen().load(Ordering::Relaxed);
                            q.retain(|j| j.gen == live);
                            if let Some(job) = q.pop_front() {
                                break job;
                            }
                            q = cv.wait(q).unwrap();
                        }
                    };
                    if job.gen != remote_thumb_gen().load(Ordering::Relaxed) {
                        continue;
                    }
                    if !file_nonempty(&job.dest) {
                        let _ = extract_remote_still(&job.url, &job.dest);
                    }
                    if job.gen != remote_thumb_gen().load(Ordering::Relaxed) {
                        continue;
                    }
                    if !file_nonempty(&job.dest) {
                        continue;
                    }
                    let child = job.child;
                    let dest = job.dest;
                    let gen = job.gen;
                    glib::idle_add_once(move || {
                        if gen != remote_thumb_gen().load(Ordering::Relaxed) {
                            return;
                        }
                        if let Some(c) = child.upgrade() {
                            refresh_remote_tile_still(&c, &dest);
                        }
                    });
                });
        }
        tx
    })
    .clone()
}

pub(crate) fn ffmpeg_http_still(url: &str, dest: &Path, ss_before_input: bool) -> bool {
    let mut cmd = ffmpeg_cmd();
    cmd.arg("-y").arg("-user_agent").arg(catalog::UA);
    if ss_before_input {
        cmd.arg("-ss").arg("1");
    }
    cmd.arg("-i").arg(url);
    if !ss_before_input {
        cmd.arg("-ss").arg("1");
    }
    cmd.arg("-an")
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg("scale=264:-2")
        .arg("-q:v")
        .arg("4")
        .arg(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };
    wait_child_timeout(&mut child, REMOTE_STILL_TIMEOUT) && file_nonempty(dest)
}

