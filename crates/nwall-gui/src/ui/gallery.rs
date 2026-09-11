
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use adw::prelude::*;
use glib::object::SendWeakRef;
use gtk::{
    glib, Align, Box as GtkBox, FlowBox, FlowBoxChild, Image, Label, Orientation,
    SelectionMode,
};
use nwall_catalog as catalog;
use nwall_ipc::{
    client_request, is_image, is_video, OutputStatus, Request, Response,
};

use crate::consts::*;
use super::discover::thumbs::{make_fixed_thumb, pack_tile};
use super::preview::bind_thumb;

pub(crate) fn compact_flow(flow: &FlowBox) {
    flow.set_selection_mode(SelectionMode::Single);
    flow.set_homogeneous(true);
    flow.set_max_children_per_line(10);
    flow.set_min_children_per_line(4);
    flow.set_row_spacing(8);
    flow.set_column_spacing(8);
    flow.set_margin_top(GALLERY_TOOLBAR_GAP);
    flow.set_margin_bottom(8);
    flow.set_margin_start(8);
    flow.set_margin_end(8);
    flow.set_hexpand(true);
    flow.set_vexpand(false);
    // FlowBox: Start, not Fill (avoids tall stretched cards).
    flow.set_valign(Align::Start);
    flow.set_halign(Align::Fill);
}

pub(crate) fn gallery_dirs(library: &Path) -> Vec<PathBuf> {
    if library.is_dir() {
        vec![library.to_path_buf()]
    } else {
        Vec::new()
    }
}

fn format_grouped_monitor_status(outputs: &[OutputStatus]) -> String {
    let mut groups: Vec<(Vec<String>, String)> = Vec::new();
    let mut group_idx: HashMap<Option<PathBuf>, usize> = HashMap::new();

    for o in outputs {
        let wp = o.wallpaper.clone();
        if let Some(&idx) = group_idx.get(&wp) {
            groups[idx].0.push(o.name.clone());
        } else {
            let display = wp
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("—")
                .to_string();
            group_idx.insert(wp, groups.len());
            groups.push((vec![o.name.clone()], display));
        }
    }

    groups
        .into_iter()
        .map(|(names, display)| format!("{}: {display}", names.join(", ")))
        .collect::<Vec<_>>()
        .join("   ")
}

pub(crate) fn refresh_status(status: &Label) {
    match client_request(&Request::Status) {
        Ok(Response::Status(s)) => status.set_text(&format_grouped_monitor_status(&s.outputs)),
        _ => status.set_text("Daemon not connected — start with: nwall daemon"),
    }
}

pub(crate) fn empty_gallery_label(text: &str) -> Label {
    let empty = Label::new(Some(text));
    empty.set_justify(gtk::Justification::Center);
    empty.set_wrap(true);
    empty.set_halign(Align::Center);
    empty.set_valign(Align::Center);
    empty.set_hexpand(true);
    empty.set_vexpand(true);
    empty.add_css_class("discover-status");
    empty.add_css_class("dim-label");
    empty
}

pub(crate) fn empty_gallery_page(text: &str) -> GtkBox {
    let page = GtkBox::new(Orientation::Vertical, 0);
    page.set_hexpand(true);
    page.set_vexpand(true);
    page.set_halign(Align::Center);
    page.set_valign(Align::Center);
    page.add_css_class("discover-loading");
    page.append(&empty_gallery_label(text));
    page
}

const CRITERIA_KEY: &str = "nwall-gallery-criteria";
const SCAN_SIG_KEY: &str = "nwall-gallery-scan-sig";
static SCAN_GEN: AtomicU64 = AtomicU64::new(0);

struct GalleryCriteria {
    kind: Cell<u32>,
    query: RefCell<String>,
}

impl GalleryCriteria {
    fn matches(&self, child: &FlowBoxChild) -> bool {
        let Some(path) = child_path(child) else {
            return true;
        };
        let kind_ok = match self.kind.get() {
            1 => !child_is_video(child),
            2 => child_is_video(child),
            _ => true,
        };
        kind_ok && path_matches_library_search(&path, &self.query.borrow())
    }
}

fn gallery_criteria(flow: &FlowBox) -> Rc<GalleryCriteria> {
    if let Some(existing) =
        unsafe { flow.data::<Rc<GalleryCriteria>>(CRITERIA_KEY).map(|c| (*c.as_ref()).clone()) }
    {
        return existing;
    }
    let crit = Rc::new(GalleryCriteria {
        kind: Cell::new(0),
        query: RefCell::new(String::new()),
    });
    unsafe {
        flow.set_data(CRITERIA_KEY, Rc::clone(&crit));
    }
    let filter_crit = Rc::clone(&crit);
    flow.set_filter_func(move |child| filter_crit.matches(child));
    crit
}

fn set_criteria(flow: &FlowBox, filter: u32, search: &str) -> (Rc<GalleryCriteria>, bool) {
    let crit = gallery_criteria(flow);
    let query = search.trim().to_lowercase();
    let changed = crit.kind.get() != filter || *crit.query.borrow() != query;
    crit.kind.set(filter);
    *crit.query.borrow_mut() = query;
    (crit, changed)
}

struct LibraryScan {
    paths: Vec<PathBuf>,
    signature: u64,
}

fn hash_bytes(h: &mut u64, bytes: impl IntoIterator<Item = u8>) {
    for b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

fn scan_library(dirs: &[PathBuf]) -> LibraryScan {
    let mut found: Vec<(PathBuf, u64, u64)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() || !(is_image(&path) || is_video(&path)) {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            found.push((path, meta.len(), mtime));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.dedup_by(|a, b| a.0 == b.0);
    let mut signature: u64 = 0xcbf29ce484222325;
    for (path, len, mtime) in &found {
        hash_bytes(&mut signature, path.to_string_lossy().bytes());
        hash_bytes(&mut signature, len.to_le_bytes());
        hash_bytes(&mut signature, mtime.to_le_bytes());
    }
    LibraryScan {
        paths: found.into_iter().map(|(p, _, _)| p).collect(),
        signature,
    }
}

fn rebuild_tiles(flow: &FlowBox, paths: &[PathBuf]) {
    while let Some(child) = flow.first_child() {
        flow.remove(&child);
    }
    for path in paths {
        flow.insert(&make_tile(path), -1);
    }
}

fn any_tile_matches(flow: &FlowBox, crit: &GalleryCriteria) -> bool {
    let mut next = flow.first_child();
    while let Some(widget) = next {
        next = widget.next_sibling();
        if let Some(child) = widget.downcast_ref::<FlowBoxChild>() {
            if crit.matches(child) {
                return true;
            }
        }
    }
    false
}

fn apply_criteria(body: &gtk::Stack, flow: &FlowBox, crit: &GalleryCriteria) {
    flow.invalidate_filter();
    if any_tile_matches(flow, crit) {
        body.set_visible_child_name("grid");
    } else {
        body.set_visible_child_name("empty");
    }
}

pub(crate) fn filter_gallery(body: &gtk::Stack, flow: &FlowBox, filter: u32, search: &str) {
    let (crit, changed) = set_criteria(flow, filter, search);
    if changed {
        flow.unselect_all();
    }
    apply_criteria(body, flow, &crit);
}

pub(crate) fn refresh_gallery(
    body: &gtk::Stack,
    flow: &FlowBox,
    dirs: &[PathBuf],
    filter: u32,
    search: &str,
) {
    set_criteria(flow, filter, search);
    let my = SCAN_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let dirs = dirs.to_vec();
    let body_w = SendWeakRef::from(body.downgrade());
    let flow_w = SendWeakRef::from(flow.downgrade());
    std::thread::spawn(move || {
        let scan = scan_library(&dirs);
        glib::idle_add_once(move || {
            if SCAN_GEN.load(Ordering::SeqCst) != my {
                return;
            }
            let (Some(body), Some(flow)) = (body_w.upgrade(), flow_w.upgrade()) else {
                return;
            };
            let known = unsafe { flow.data::<u64>(SCAN_SIG_KEY).map(|s| *s.as_ref()) };
            if known == Some(scan.signature) {
                flow.unselect_all();
            } else {
                rebuild_tiles(&flow, &scan.paths);
                unsafe {
                    flow.set_data(SCAN_SIG_KEY, scan.signature);
                }
            }
            let crit = gallery_criteria(&flow);
            apply_criteria(&body, &flow, &crit);
        });
    });
}

pub(crate) fn path_matches_library_search(path: &Path, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    let fname = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if fname.contains(query_lower) {
        return true;
    }
    catalog::library_caption(path)
        .to_lowercase()
        .contains(query_lower)
}

pub(crate) fn kind_badge(video: bool) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 0);
    row.add_css_class(if video { "kind-badge" } else { "kind-badge-image" });
    row.set_valign(Align::Start);
    row.set_halign(Align::End);
    row.set_hexpand(false);
    row.set_vexpand(false);
    row.set_margin_top(5);
    row.set_margin_end(5);
    row.set_tooltip_text(Some(if video { "Video" } else { "Image" }));
    let icon = Image::from_icon_name(if video {
        "camera-video-symbolic"
    } else {
        "image-x-generic-symbolic"
    });
    icon.set_hexpand(true);
    icon.set_vexpand(true);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    row.append(&icon);
    row
}

pub(crate) fn make_tile(path: &Path) -> FlowBoxChild {
    let (host, pic) = make_fixed_thumb();
    let video = is_video(path);
    bind_thumb(&pic, path.to_path_buf(), video);
    host.add_overlay(&kind_badge(video));
    let title = catalog::library_caption(path);
    let child = pack_tile(&host, &title);
    unsafe {
        child.set_data("nwall-path", path.to_path_buf());
        child.set_data("nwall-video", video);
        child.set_data("nwall-tile-pic", pic);
        child.set_data("nwall-tile-host", host);
    }
    child
}

pub(crate) fn child_path(child: &FlowBoxChild) -> Option<PathBuf> {
    unsafe { child.data::<PathBuf>("nwall-path").map(|p| (*p.as_ref()).clone()) }
}

fn child_is_video(child: &FlowBoxChild) -> bool {
    unsafe { child.data::<bool>("nwall-video").map(|v| *v.as_ref()) }.unwrap_or(false)
}
