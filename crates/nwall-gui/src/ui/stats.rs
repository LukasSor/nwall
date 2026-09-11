
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use adw::prelude::*;
use glib::object::SendWeakRef;
use gtk::{
    gdk, gio, glib, Align, Box as GtkBox, FlowBox, FlowBoxChild, Grid, Label, Orientation, Overlay,
};
use nwall_catalog as catalog;
use nwall_ipc::{is_video};

use crate::consts::*;
use crate::ui::discover::remote_child_item;
use crate::ui::preview::{apply_sidebar_wrap, file_nonempty, set_preview_title};

#[derive(Clone)]
pub(crate) struct StatsPane {
    pub(crate) root: GtkBox,
    pub(crate) content: GtkBox,
    pub(crate) avatar_gen: Arc<AtomicU64>,
}

#[derive(Clone)]
pub(crate) struct StatsPaneRefs {
    pub(crate) content: SendWeakRef<GtkBox>,
    pub(crate) avatar_gen: Arc<AtomicU64>,
}

impl StatsPane {
    pub(crate) fn new() -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(false);
        root.set_valign(Align::Start);
        root.set_overflow(gtk::Overflow::Visible);
        root.add_css_class("preview-stats");

        let content = GtkBox::new(Orientation::Vertical, 14);
        content.set_hexpand(true);
        content.set_valign(Align::Start);
        content.set_overflow(gtk::Overflow::Visible);
        content.add_css_class("preview-stats-content");
        root.append(&content);
        root.set_visible(true);

        Self {
            root,
            content,
            avatar_gen: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn refs(&self) -> StatsPaneRefs {
        StatsPaneRefs {
            content: SendWeakRef::from(self.content.downgrade()),
            avatar_gen: Arc::clone(&self.avatar_gen),
        }
    }

    pub(crate) fn apply(&self, stats: &catalog::MediaStats) {
        self.apply_inner(stats, false);
    }

    pub(crate) fn apply_inner(&self, stats: &catalog::MediaStats, pending: bool) {
        apply_stats_widgets(&self.content, &self.avatar_gen, stats, pending);
    }

    pub(crate) fn clear(&self) {
        self.apply(&catalog::MediaStats::default());
    }
}

impl StatsPaneRefs {
    pub(crate) fn apply(&self, stats: &catalog::MediaStats) {
        self.apply_inner(stats, false);
    }

    pub(crate) fn apply_pending(&self, stats: &catalog::MediaStats) {
        self.apply_inner(stats, true);
    }

    pub(crate) fn apply_inner(&self, stats: &catalog::MediaStats, pending: bool) {
        let Some(content) = self.content.upgrade() else {
            return;
        };
        apply_stats_widgets(&content, &self.avatar_gen, stats, pending);
    }
}

pub(crate) fn stats_section_header(title: &str) -> Label {
    let header = Label::new(Some(title));
    header.add_css_class("stats-section-title");
    header.set_halign(Align::Start);
    header.set_hexpand(true);
    header.set_xalign(0.0);
    header.set_justify(gtk::Justification::Left);
    header.set_wrap(false);
    header.set_single_line_mode(true);
    header.set_ellipsize(gtk::pango::EllipsizeMode::None);
    header.set_max_width_chars(-1);
    header
}

pub(crate) fn stats_prop_label(text: &str) -> Label {
    let lbl = Label::new(Some(text));
    lbl.add_css_class("stats-prop-label");
    lbl.set_halign(Align::Start);
    lbl.set_valign(Align::Start);
    lbl.set_xalign(0.0);
    lbl.set_justify(gtk::Justification::Left);
    lbl.set_hexpand(false);
    lbl.set_hexpand_set(true);
    lbl.set_wrap(false);
    lbl.set_single_line_mode(true);
    lbl.set_ellipsize(gtk::pango::EllipsizeMode::None);
    lbl.set_max_width_chars(-1);
    lbl
}

const STATS_SECTION_GAP: i32 = 10;

fn stats_table_grid() -> Grid {
    let rows = Grid::new();
    rows.set_column_spacing(STATS_COL_GAP);
    rows.set_row_spacing(4);
    rows.set_hexpand(true);
    rows.set_halign(Align::Fill);
    rows.set_column_homogeneous(false);
    rows.set_overflow(gtk::Overflow::Visible);
    rows.add_css_class("stats-section-rows");
    rows.add_css_class("stats-table");
    rows
}

struct StatsTable {
    root: GtkBox,
    grid: Grid,
    row: i32,
    pending_header: Option<&'static str>,
    labels: Vec<Label>,
}

impl StatsTable {
    fn new() -> Self {
        let root = GtkBox::new(Orientation::Vertical, 4);
        root.set_hexpand(true);
        root.set_halign(Align::Fill);
        root.set_valign(Align::Start);
        Self {
            root,
            grid: stats_table_grid(),
            row: 0,
            pending_header: None,
            labels: Vec::new(),
        }
    }

    fn section(&mut self, title: &'static str) {
        self.pending_header = Some(title);
    }

    fn finish_grid(&mut self) {
        if self.row == 0 {
            return;
        }
        self.root.append(&self.grid);
        self.grid = stats_table_grid();
        self.row = 0;
    }

    fn flush_header(&mut self) {
        let Some(title) = self.pending_header.take() else {
            return;
        };
        self.finish_grid();
        let header = stats_section_header(title);
        if self.root.first_child().is_some() {
            header.set_margin_top(STATS_SECTION_GAP);
        }
        self.root.append(&header);
    }

    fn append_prop(&mut self, label: &str, value: Option<String>) -> bool {
        let Some(v) = value.filter(|s| !s.trim().is_empty()) else {
            return false;
        };
        self.flush_header();
        self.attach_stats_value(label, &stats_prop_value(v.trim()));
        true
    }

    fn attach_value(&mut self, label: &str, value: &impl IsA<gtk::Widget>) {
        self.flush_header();
        self.attach_stats_value(label, value);
    }

    fn attach_stats_value(&mut self, label: &str, value: &impl IsA<gtk::Widget>) {
        let lbl = stats_prop_label(label);
        self.labels.push(lbl.clone());
        self.grid.attach(&lbl, 0, self.row, 1, 1);
        value.set_hexpand(true);
        value.set_hexpand_set(true);
        value.set_halign(Align::Fill);
        self.grid.attach(value, 1, self.row, 1, 1);
        self.row += 1;
    }

    fn attach_full(&mut self, widget: &impl IsA<gtk::Widget>) {
        self.flush_header();
        self.finish_grid();
        widget.set_hexpand(true);
        widget.set_hexpand_set(true);
        widget.set_halign(Align::Fill);
        self.root.append(widget);
    }

    fn take(mut self) -> GtkBox {
        self.finish_grid();
        if !self.labels.is_empty() {
            bind_stats_column_split(&self.root, self.labels);
        }
        self.root
    }
}

/// Column 0 width: `max(natural labels, sidebar_width/2 − gap/2)`.
/// `sidebar_width` is the `preview-sidebar` allocation (`bind_preview_host_size`).
fn stats_label_col_width(sidebar_w: i32, natural: i32, gap: i32) -> i32 {
    let half = (sidebar_w / 2 - gap / 2).max(0);
    natural.max(half)
}

fn natural_label_width(label: &Label) -> i32 {
    let prev = label.width_request();
    label.set_width_request(-1);
    let (_, nat, _, _) = label.measure(gtk::Orientation::Horizontal, -1);
    label.set_width_request(prev);
    nat.max(1)
}

fn max_natural_label_width(labels: &[Label]) -> i32 {
    labels.iter().map(natural_label_width).max().unwrap_or(1)
}

fn apply_stats_label_col(labels: &[Label], col0: i32) {
    for lbl in labels {
        if lbl.width_request() != col0 {
            lbl.set_width_request(col0);
        }
    }
}

fn bind_stats_column_split(root: &GtkBox, labels: Vec<Label>) {
    let labels = Rc::new(labels);
    let last_w = Rc::new(Cell::new(0i32));
    let apply = {
        let labels = Rc::clone(&labels);
        let last_w = Rc::clone(&last_w);
        let root = root.clone();
        Rc::new(move || {
            let measured = stats_split_width(&root);
            let width = if measured > 8 { measured } else { SIDEBAR_MIN };
            if last_w.get() == width {
                return;
            }
            last_w.set(width);
            let nat = max_natural_label_width(&labels);
            let col0 = stats_label_col_width(width, nat, STATS_COL_GAP as i32);
            apply_stats_label_col(&labels, col0);
        }) as Rc<dyn Fn()>
    };

    apply();

    root.add_tick_callback({
        let apply = Rc::clone(&apply);
        move |_, _| {
            apply();
            glib::ControlFlow::Continue
        }
    });

    root.connect_map({
        let apply = Rc::clone(&apply);
        move |root| {
            hook_sidebar_width(root, SidebarWidthKind::Stats, &apply);
            apply();
        }
    });
}

fn bind_stats_value_wrap(label: &Label) {
    let last = Rc::new(Cell::new(0i32));
    label.connect_notify_local(Some("width"), move |l, _| {
        let width = l.width();
        if width <= 8 || last.get() == width {
            return;
        }
        last.set(width);
        let chars = (width / 7).clamp(8, 96);
        if l.max_width_chars() != chars {
            l.set_max_width_chars(chars);
        }
    });
}

fn stats_prop_value(text: &str) -> Label {
    let val = Label::new(Some(text));
    val.add_css_class("stats-prop-value");
    val.set_selectable(true);
    apply_sidebar_wrap(&val, false);
    val.set_halign(Align::Fill);
    val.set_valign(Align::Start);
    val.set_xalign(0.0);
    val.set_justify(gtk::Justification::Left);
    bind_stats_value_wrap(&val);
    val
}

fn attach_stats_link(table: &mut StatsTable, url: &str) {
    let escaped = glib::markup_escape_text(url);
    let val = Label::new(None);
    val.set_markup(&format!("<a href=\"{escaped}\">Open page</a>"));
    val.add_css_class("stats-prop-value");
    val.add_css_class("preview-stats-link");
    apply_sidebar_wrap(&val, false);
    val.set_halign(Align::Fill);
    val.set_valign(Align::Start);
    val.set_xalign(0.0);
    val.set_justify(gtk::Justification::Left);
    bind_stats_value_wrap(&val);
    table.attach_value("Link", &val);
}

fn attach_stats_uploader(
    table: &mut StatsTable,
    label: &str,
    name: &str,
    avatar_url: Option<&str>,
    avatar_gen: &Arc<AtomicU64>,
) {
    let avatar = adw::Avatar::new(AVATAR_PX, Some(name), true);
    avatar.add_css_class("uploader-avatar");
    avatar.set_valign(Align::Start);
    let has_url = avatar_url.map(str::trim).is_some_and(|s| !s.is_empty());
    if has_url {
        bind_avatar(&avatar, avatar_url, avatar_gen);
    } else {
        avatar.set_visible(true);
    }

    let name_l = stats_prop_value(name);

    let value = GtkBox::new(Orientation::Horizontal, 8);
    value.add_css_class("stats-uploader-row");
    value.set_halign(Align::Fill);
    value.set_hexpand(true);
    value.set_valign(Align::Start);
    value.append(&avatar);
    value.append(&name_l);
    table.attach_value(label, &value);
}

pub(crate) fn stats_colors_block(hexes: &[&str]) -> GtkBox {
    let swatches = GtkBox::new(Orientation::Horizontal, 4);
    swatches.set_halign(Align::Start);
    swatches.set_hexpand(true);
    swatches.add_css_class("stats-section");
    swatches.add_css_class("stats-colors");
    swatches.add_css_class("color-swatches");
    swatches.add_css_class("stats-block-body");
    for hex in hexes {
        swatches.append(&color_swatch(hex));
    }
    swatches
}

pub(crate) fn stats_tags_block(tags: &[String]) -> GtkBox {
    let cloud = GtkBox::new(Orientation::Vertical, TAG_ROW_GAP);
    cloud.set_halign(Align::Fill);
    cloud.set_hexpand(true);
    cloud.set_hexpand_set(true);
    cloud.set_valign(Align::Start);
    cloud.set_vexpand(false);
    cloud.set_overflow(gtk::Overflow::Visible);
    cloud.add_css_class("stats-section");
    cloud.add_css_class("stats-tags");
    cloud.add_css_class("preview-tags-cloud");
    cloud.add_css_class("stats-block-body");

    let chips: Vec<GtkBox> = tags.iter().map(|t| tag_chip(t)).collect();
    for chip in &chips {
        chip.set_halign(Align::Start);
    }
    bind_centered_tag_wrap(&cloud, chips);
    cloud
}

fn tag_chip_label(chip: &GtkBox) -> Option<Label> {
    chip.first_child()?.downcast::<Label>().ok()
}

fn tag_chip_width(chip: &GtkBox) -> i32 {
    let (_, nat, _, _) = chip.measure(gtk::Orientation::Horizontal, -1);
    nat.max(1)
}

fn apply_chip_width_limit(chips: &[GtkBox], width: i32) {
    let chars = if width > 8 {
        (width / 7).clamp(4, 80)
    } else {
        -1
    };
    for chip in chips {
        let Some(label) = tag_chip_label(chip) else {
            continue;
        };
        chip.set_size_request(-1, -1);
        label.set_ellipsize(gtk::pango::EllipsizeMode::None);
        label.set_max_width_chars(-1);
        let nat = tag_chip_width(chip);
        if width > 8 && nat > width {
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(chars);
        }
    }
}

fn nearest_css_class(start: &impl IsA<gtk::Widget>, class: &str) -> Option<gtk::Widget> {
    let mut widget = Some(start.as_ref().clone());
    while let Some(p) = widget {
        if p.has_css_class(class) {
            return Some(p);
        }
        widget = p.parent();
    }
    None
}

fn preview_sidebar_width(start: &impl IsA<gtk::Widget>) -> i32 {
    // Same allocation `bind_preview_host_size` uses.
    if let Some(side) = nearest_css_class(start, "preview-sidebar") {
        let w = side.width();
        if w > 8 {
            return w;
        }
    }
    0
}

fn stats_split_width(start: &impl IsA<gtk::Widget>) -> i32 {
    let side = preview_sidebar_width(start);
    if side > 8 {
        return side;
    }
    let own = start.as_ref().width();
    if own > 8 {
        return own;
    }
    if let Some(scroll) = nearest_css_class(start, "preview-stats-scroll") {
        let w = scroll.width();
        if w > 8 {
            return w;
        }
    }
    0
}

fn tag_layout_width(start: &impl IsA<gtk::Widget>) -> i32 {
    // Same allocation bind_preview_host_size uses, minus sidebar margins.
    let side = preview_sidebar_width(start);
    if side > 8 {
        return (side - 24).max(8);
    }
    let own = start.as_ref().width();
    if own > 8 {
        return own;
    }
    if let Some(scroll) = nearest_css_class(start, "preview-stats-scroll") {
        let w = scroll.width();
        if w > 8 {
            return w;
        }
    }
    0
}

fn greedy_tag_row_counts(widths: &[i32], avail: i32, gap: i32) -> Vec<usize> {
    let mut counts = Vec::new();
    let mut row_w = 0;
    let mut row_n = 0usize;
    for &w in widths {
        let need = if row_n == 0 { w } else { row_w + gap + w };
        if row_n > 0 && need > avail {
            counts.push(row_n);
            row_w = w;
            row_n = 1;
        } else {
            row_w = need;
            row_n += 1;
        }
    }
    if row_n > 0 {
        counts.push(row_n);
    }
    counts
}

fn tag_row_counts(chips: &[GtkBox], width: i32, gap: i32) -> Vec<usize> {
    let widths: Vec<i32> = chips.iter().map(tag_chip_width).collect();
    greedy_tag_row_counts(&widths, width, gap)
}

fn chip_row_height(chips: &[GtkBox]) -> i32 {
    chips
        .first()
        .map(|c| {
            let (_, nat, _, _) = c.measure(gtk::Orientation::Vertical, -1);
            nat.max(1)
        })
        .unwrap_or(1)
}

fn make_centered_tag_row(height: i32) -> (Overlay, GtkBox) {
    let host = Overlay::new();
    host.set_hexpand(true);
    host.set_halign(Align::Fill);
    host.set_valign(Align::Start);
    host.set_overflow(gtk::Overflow::Visible);
    host.add_css_class("preview-tags-row");

    // Overlay chips are not measured, so row min-width stays ~0 and the
    // paned can shrink. Height comes from this sizer only.
    let sizer = GtkBox::new(Orientation::Horizontal, 0);
    sizer.set_hexpand(true);
    sizer.set_halign(Align::Fill);
    sizer.set_size_request(0, height);
    host.set_child(Some(&sizer));

    let row = GtkBox::new(Orientation::Horizontal, TAG_ROW_GAP);
    row.set_halign(Align::Center);
    row.set_valign(Align::Center);
    row.set_hexpand(false);
    host.add_overlay(&row);
    host.set_measure_overlay(&row, false);
    (host, row)
}

fn unparent_tag_chip(chip: &GtkBox) {
    if let Some(parent) = chip.parent() {
        if let Ok(row) = parent.downcast::<GtkBox>() {
            row.remove(chip);
        }
    }
}

fn relayout_centered_tags(cloud: &GtkBox, chips: &[GtkBox], width: i32) {
    apply_chip_width_limit(chips, width);
    let counts = tag_row_counts(chips, width, TAG_ROW_GAP);
    for chip in chips {
        unparent_tag_chip(chip);
    }
    while let Some(child) = cloud.first_child() {
        cloud.remove(&child);
    }
    let height = chip_row_height(chips);
    let mut i = 0usize;
    for n in &counts {
        let (host, row) = make_centered_tag_row(height);
        for _ in 0..*n {
            row.append(&chips[i]);
            i += 1;
        }
        cloud.append(&host);
    }
}

fn bind_centered_tag_wrap(cloud: &GtkBox, chips: Vec<GtkBox>) {
    if chips.is_empty() {
        return;
    }
    let chips = Rc::new(chips);
    let last_w = Rc::new(Cell::new(0i32));
    let apply = {
        let chips = Rc::clone(&chips);
        let last_w = Rc::clone(&last_w);
        let cloud = cloud.clone();
        Rc::new(move || {
            let measured = tag_layout_width(&cloud);
            let width = if measured > 8 { measured } else { SIDEBAR_MIN };
            if last_w.get() == width {
                return;
            }
            last_w.set(width);
            relayout_centered_tags(&cloud, &chips, width);
            cloud.queue_resize();
        }) as Rc<dyn Fn()>
    };

    apply();

    cloud.add_tick_callback({
        let apply = Rc::clone(&apply);
        move |_, _| {
            apply();
            glib::ControlFlow::Continue
        }
    });

    cloud.connect_map({
        let apply = Rc::clone(&apply);
        move |cloud| {
            hook_sidebar_width(cloud, SidebarWidthKind::Tags, &apply);
            apply();
        }
    });
}

enum SidebarWidthKind {
    Tags,
    Stats,
}

#[derive(Clone, Default)]
struct SidebarWidthHooks {
    tags: Option<Rc<dyn Fn()>>,
    stats: Option<Rc<dyn Fn()>>,
}

fn hook_sidebar_width(widget: &impl IsA<gtk::Widget>, kind: SidebarWidthKind, apply: &Rc<dyn Fn()>) {
    let Some(side) = nearest_css_class(widget, "preview-sidebar") else {
        return;
    };
    let slot = sidebar_width_hooks(&side);
    {
        let mut hooks = slot.borrow_mut();
        match kind {
            SidebarWidthKind::Tags => hooks.tags = Some(Rc::clone(apply)),
            SidebarWidthKind::Stats => hooks.stats = Some(Rc::clone(apply)),
        }
    }
    let connect = unsafe { side.data::<bool>("nwall-sidebar-width-sig").is_none() };
    if !connect {
        return;
    }
    unsafe {
        side.set_data("nwall-sidebar-width-sig", true);
    }
    let slot_n = Rc::clone(&slot);
    side.connect_notify_local(Some("width"), move |_, _| {
        let (tags, stats) = {
            let hooks = slot_n.borrow();
            (hooks.tags.clone(), hooks.stats.clone())
        };
        if let Some(run) = tags {
            run();
        }
        if let Some(run) = stats {
            run();
        }
    });
}

fn sidebar_width_hooks(side: &gtk::Widget) -> Rc<RefCell<SidebarWidthHooks>> {
    unsafe {
        if let Some(slot) = side.data::<Rc<RefCell<SidebarWidthHooks>>>("nwall-sidebar-width") {
            return slot.as_ref().clone();
        }
        let slot = Rc::new(RefCell::new(SidebarWidthHooks::default()));
        side.set_data("nwall-sidebar-width", slot.clone());
        slot
    }
}

pub(crate) fn stats_source_name(stats: &catalog::MediaStats) -> Option<String> {
    nonempty_gui(&stats.source)
        .map(str::to_string)
        .or_else(|| {
            nonempty_gui(&stats.credit).map(|c| catalog::canonical_source_label(c, None))
        })
}

pub(crate) fn stats_credit_display(stats: &catalog::MediaStats, source: Option<&str>) -> Option<String> {
    let c = nonempty_gui(&stats.credit)?;
    if source.is_some_and(|s| c.eq_ignore_ascii_case(s)) {
        return None;
    }
    let l = c.to_ascii_lowercase();
    if matches!(
        l.as_str(),
        "wallhaven"
            | "pixabay"
            | "coverr"
            | "bing"
            | "bing daily"
            | "nasa apod"
            | "internet archive"
            | "archive.org"
            | "live wallpapers"
            | "live wallpaper"
            | "video wallpapers"
            | "video wallpaper"
            | "github"
            | "video from coverr"
    ) || l.starts_with("github ")
    {
        return None;
    }
    Some(c.to_string())
}

pub(crate) fn stats_short_type(raw: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return String::new();
    }
    let lower = s.to_ascii_lowercase();
    let leaf = lower.rsplit(['/', '.']).next().unwrap_or(&lower);
    match leaf {
        "jpeg" | "jpg" => "JPEG".into(),
        "png" => "PNG".into(),
        "webp" => "WebP".into(),
        "bmp" => "BMP".into(),
        "gif" => "GIF".into(),
        "mp4" | "m4v" => "MP4".into(),
        "webm" => "WebM".into(),
        "mkv" => "MKV".into(),
        "mov" => "MOV".into(),
        "avi" => "AVI".into(),
        "h264" | "avc" => "H.264".into(),
        "hevc" | "h265" => "H.265".into(),
        "vp9" => "VP9".into(),
        "vp8" => "VP8".into(),
        "av1" => "AV1".into(),
        other if other.len() <= 8 => other.to_ascii_uppercase(),
        _ => s.to_string(),
    }
}

pub(crate) fn stats_fps_display(fps: f32) -> String {
    if (fps - fps.round()).abs() < 0.05 {
        format!("{:.0}", fps.round())
    } else {
        format!("{fps:.2}")
    }
}

pub(crate) fn apply_stats_widgets(
    content: &GtkBox,
    avatar_gen: &Arc<AtomicU64>,
    stats: &catalog::MediaStats,
    pending: bool,
) {
    while let Some(ch) = content.first_child() {
        content.remove(&ch);
    }

    let mut table = StatsTable::new();

    table.section("Media");
    let resolution = match (stats.width, stats.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some(format!("{w}×{h}")),
        _ if pending => Some("…".into()),
        _ => None,
    };
    table.append_prop("Resolution", resolution);
    let size = stats
        .file_size
        .filter(|n| *n > 0)
        .map(catalog::human_bytes)
        .or_else(|| pending.then(|| "…".into()));
    table.append_prop("Size", size);
    let duration = stats
        .duration_secs
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(catalog::format_duration);
    table.append_prop("Duration", duration);
    if let Some(fps) = stats.fps.filter(|f| *f > 0.0) {
        table.append_prop("FPS", Some(stats_fps_display(fps)));
    }
    if let Some(t) = nonempty_gui(&stats.file_type) {
        table.append_prop("Type", Some(stats_short_type(t)));
    }

    table.section("Source");
    let source = stats_source_name(stats);
    table.append_prop("Source", source.clone());
    if let Some(t) = nonempty_gui(&stats.title) {
        let skip = catalog::tag_caption(&stats.tags, 8).as_deref() == Some(t);
        if !skip {
            table.append_prop("Title", Some(t.to_string()));
        }
    }
    table.append_prop("Credit", stats_credit_display(stats, source.as_deref()));
    if let Some(r) = nonempty_gui(&stats.repo) {
        table.append_prop("Repo", Some(r.to_string()));
    }
    if nonempty_gui(&stats.repo).is_some() {
        if let Some(id) = nonempty_gui(&stats.source_id) {
            table.append_prop("Path", Some(id.to_string()));
        }
    }
    if let Some(u) = nonempty_gui(&stats.page_url) {
        attach_stats_link(&mut table, u);
    }

    table.section("Details");
    if let Some(c) = nonempty_gui(&stats.category) {
        table.append_prop("Category", Some(catalog::title_case_ascii(c)));
    }
    if let Some(p) = nonempty_gui(&stats.purity) {
        table.append_prop("Purity", Some(p.to_ascii_uppercase()));
    }
    if let Some(n) = stats.views {
        table.append_prop("Views", Some(n.to_string()));
    }
    if let Some(n) = stats.favorites {
        let fav_label = if stats_source_name(stats)
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("Pixabay"))
        {
            "Likes"
        } else {
            "Favorites"
        };
        table.append_prop(fav_label, Some(n.to_string()));
    }
    if let Some(n) = stats.downloads.filter(|n| *n > 0) {
        table.append_prop("Downloads", Some(n.to_string()));
    }
    if let Some(n) = stats.comments.filter(|n| *n > 0) {
        table.append_prop("Comments", Some(n.to_string()));
    }
    if let Some(d) = nonempty_gui(&stats.date) {
        table.append_prop("Date", Some(d.to_string()));
    }
    if let Some(c) = nonempty_gui(&stats.collection) {
        table.append_prop("Collection", Some(c.to_string()));
    }
    if let Some(l) = nonempty_gui(&stats.license) {
        table.append_prop("License", Some(l.to_string()));
    }
    if let Some(d) = nonempty_gui(&stats.description) {
        table.append_prop("Description", Some(d.to_string()));
    }
    if let Some(name) = nonempty_gui(&stats.uploader) {
        let label = if nonempty_gui(&stats.repo).is_some() {
            "Committed by"
        } else if stats_source_name(stats)
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("Archive.org"))
        {
            "Creator"
        } else {
            "Uploader"
        };
        attach_stats_uploader(
            &mut table,
            label,
            name,
            stats.avatar.as_deref(),
            avatar_gen,
        );
    }

    let colors: Vec<&str> = stats
        .colors
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .take(8)
        .collect();
    if !colors.is_empty() {
        table.section("Colors");
        table.attach_full(&stats_colors_block(&colors));
    }

    let tags: Vec<String> = stats
        .tags
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .take(TAG_CLOUD_MAX)
        .map(|t| t.to_string())
        .collect();
    if !tags.is_empty() {
        table.section("Tags");
        table.attach_full(&stats_tags_block(&tags));
    }

    let body = table.take();
    if body.first_child().is_some() {
        content.append(&body);
    }

    content.set_visible(true);
}

pub(crate) fn nonempty_gui(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

pub(crate) fn tag_name_hash(name: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h ^= u32::from(b.to_ascii_lowercase());
        h = h.wrapping_mul(16777619);
    }
    h
}

pub(crate) fn tag_bg_rgb(name: &str) -> (u8, u8, u8) {
    let h = tag_name_hash(name);
    let hue = (h % 360) as f32;
    let sat = 0.26 + ((h >> 8) % 16) as f32 / 100.0;
    let dark_ui = adw::StyleManager::default().is_dark();
    let (light_lo, light_hi) = if dark_ui {
        (0.38, 0.49)
    } else {
        (0.78, 0.90)
    };
    let span = light_hi - light_lo;
    let light = light_lo + ((h >> 16) % 12) as f32 / 100.0 * span;
    hsl_to_rgb(hue, sat, light)
}

fn tag_fg_rgb(bg: (u8, u8, u8)) -> (u8, u8, u8) {
    let (r, g, b) = bg;
    let lum = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if lum > 150.0 {
        (32, 32, 32)
    } else {
        (255, 255, 255)
    }
}

pub(crate) fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to_u8 = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    (to_u8(r1), to_u8(g1), to_u8(b1))
}

pub(crate) fn tag_chip(text: &str) -> GtkBox {
    let t = text.trim().trim_start_matches('#');
    let (br, bg, bb) = tag_bg_rgb(t);
    let (fr, fg, fb) = tag_fg_rgb((br, bg, bb));
    let chip = GtkBox::new(Orientation::Horizontal, 0);
    chip.add_css_class("preview-tag");
    chip.set_halign(Align::Start);
    chip.set_valign(Align::Center);
    chip.set_hexpand(false);
    chip.set_vexpand(false);
    chip.set_overflow(gtk::Overflow::Hidden);

    let l = Label::new(Some(t));
    l.add_css_class("preview-tag-label");
    l.set_halign(Align::Center);
    l.set_valign(Align::Center);
    l.set_hexpand(false);
    l.set_wrap(false);
    l.set_single_line_mode(true);
    l.set_ellipsize(gtk::pango::EllipsizeMode::None);
    l.set_max_width_chars(-1);
    l.set_justify(gtk::Justification::Center);
    chip.append(&l);

    let class = format!("nwall-tag-{:08x}", tag_name_hash(t));
    apply_local_css(
        &chip,
        &class,
        &format!(
            "box.preview-tag.{class} {{\n\
             background-color: rgba({br},{bg},{bb},0.62);\n\
             border-radius: 999px;\n\
             padding: 2px 9px;\n\
             border: none;\n\
            }}\n\
             box.preview-tag.{class} label {{\n\
             color: rgba({fr},{fg},{fb},0.88);\n\
             font-size: 0.82em;\n\
             font-weight: 500;\n\
             white-space: nowrap;\n\
            }}"
        ),
    );
    chip
}

pub(crate) fn color_swatch(hex: &str) -> GtkBox {
    let swatch = GtkBox::new(Orientation::Horizontal, 0);
    swatch.add_css_class("color-swatch");
    swatch.set_halign(Align::Start);
    swatch.set_valign(Align::Center);
    swatch.set_hexpand(false);
    swatch.set_vexpand(false);
    swatch.set_size_request(18, 18);
    swatch.set_overflow(gtk::Overflow::Hidden);
    swatch.set_tooltip_text(Some(hex));
    if let Some((r, g, b)) = parse_css_hex(hex) {
        let class = format!(
            "nwall-swatch-{}",
            hex.trim().trim_start_matches('#').to_ascii_lowercase()
        );
        apply_local_css(
            &swatch,
            &class,
            &format!(
                "box.color-swatch.{class} {{\n\
                 min-width: 18px;\n\
                 min-height: 18px;\n\
                 background-color: rgb({r},{g},{b});\n\
                 border: none;\n\
                 outline: none;\n\
                 border-radius: 2px;\n\
                 box-shadow: none;\n\
                }}"
            ),
        );
    }
    swatch
}

pub(crate) fn parse_css_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().trim_start_matches('#');
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        let r = u8::from_str_radix(&h[0..2], 16).ok()?;
        let g = u8::from_str_radix(&h[2..4], 16).ok()?;
        let b = u8::from_str_radix(&h[4..6], 16).ok()?;
        return Some((r, g, b));
    }
    None
}

thread_local! {
    static LOCAL_CSS_PROVIDERS: RefCell<HashMap<String, gtk::CssProvider>> =
        RefCell::new(HashMap::new());
}

pub(crate) fn apply_local_css(widget: &impl IsA<gtk::Widget>, class: &str, css: &str) {
    widget.add_css_class(class);
    LOCAL_CSS_PROVIDERS.with(|providers| {
        let mut providers = providers.borrow_mut();
        if let Some(provider) = providers.get(class) {
            provider.load_from_string(css);
            return;
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
        providers.insert(class.to_string(), provider);
    });
}

pub(crate) fn bind_avatar(avatar: &adw::Avatar, url: Option<&str>, gen: &Arc<AtomicU64>) {
    let my = gen.fetch_add(1, Ordering::Relaxed) + 1;
    avatar.set_custom_image(Option::<&gdk::Paintable>::None);
    avatar.set_visible(false);
    let Some(url) = url.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let dest = catalog::cached_path(url, "avatar");
    if file_nonempty(&dest) {
        set_avatar_from_path(avatar, &dest);
        return;
    }
    let url = url.to_string();
    let weak = SendWeakRef::from(avatar.downgrade());
    let gen = Arc::clone(gen);
    std::thread::spawn(move || {
        let Ok(path) = catalog::download(&url, "avatar") else {
            return;
        };
        glib::idle_add_once(move || {
            if gen.load(Ordering::Relaxed) != my {
                return;
            }
            if let Some(av) = weak.upgrade() {
                set_avatar_from_path(&av, &path);
            }
        });
    });
}

pub(crate) fn set_avatar_from_path(avatar: &adw::Avatar, path: &Path) {
    let Ok(bytes) = std::fs::read(path) else {
        avatar.set_visible(false);
        return;
    };
    if bytes.len() < 24 || bytes.starts_with(b"<") || bytes.starts_with(b"<!") {
        avatar.set_visible(false);
        return;
    }
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from(&bytes));
    match gdk_pixbuf::Pixbuf::from_stream(&stream, gio::Cancellable::NONE) {
        Ok(pb) => {
            let pb = pb
                .scale_simple(
                    AVATAR_PX * 2,
                    AVATAR_PX * 2,
                    gdk_pixbuf::InterpType::Bilinear,
                )
                .unwrap_or(pb);
            avatar.set_custom_image(Some(&gdk::Texture::for_pixbuf(&pb)));
            avatar.set_visible(true);
        }
        Err(_) => avatar.set_visible(false),
    }
}

pub(crate) fn make_stats_pane() -> StatsPane {
    StatsPane::new()
}

pub(crate) fn schedule_local_stats(
    path: PathBuf,
    extra: Option<catalog::MediaStats>,
    stats: &StatsPaneRefs,
    gen: &Arc<AtomicU64>,
    my: u64,
) {
    let mut immediate = extra.unwrap_or_default();
    if let Some(meta) = catalog::load_path_meta(&path) {
        immediate.fill_from(&meta);
    }
    let pending = {
        let mut p = immediate.clone();
        if p.source.is_none() {
            p.source = Some("Library".into());
        }
        p
    };
    stats.apply_pending(&pending);
    let refs = stats.clone();
    let gen = Arc::clone(gen);
    std::thread::spawn(move || {
        let mut s = immediate;
        if let Some(meta) = catalog::enrich_library_meta(&path) {
            s.fill_from(&meta);
            fill_gui_source(&mut s, &meta);
        } else if let Some(meta) = catalog::load_path_meta(&path) {
            s.fill_from(&meta);
            fill_gui_source(&mut s, &meta);
        }
        let mut probe = catalog::probe_file_stats(&path);
        if probe.width.unwrap_or(0) == 0 && !is_video(&path) {
            if let Some((_, w, h)) = gdk_pixbuf::Pixbuf::file_info(&path) {
                if w > 0 && h > 0 {
                    probe.width = Some(w as u32);
                    probe.height = Some(h as u32);
                }
            }
        }
        s.merge_probe(&probe);
        if s.source.is_none() {
            s.source = Some("Library".into());
        }
        glib::idle_add_once(move || {
            if gen.load(Ordering::Relaxed) != my {
                return;
            }
            refs.apply(&s);
        });
    });
}

pub(crate) fn fill_gui_source(stats: &mut catalog::MediaStats, meta: &catalog::MediaStats) {
    if let Some(src) = meta
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "Library")
    {
        stats.source = Some(src.to_string());
        return;
    }
    if let Some(c) = meta.credit.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        stats.source = Some(catalog::canonical_source_label(c, meta.page_url.as_deref()));
    }
}

pub(crate) struct WallhavenEnrichJob {
    pub(crate) id: String,
    pub(crate) child: SendWeakRef<FlowBoxChild>,
}

pub(crate) fn enrich_wallhaven_tiles(
    flow: &FlowBox,
    api_key: &str,
    gen: Arc<AtomicU64>,
    my: u64,
    pending: Arc<Mutex<Option<catalog::RemoteItem>>>,
    preview_name: &Label,
    stats: &StatsPaneRefs,
) {
    let mut jobs = Vec::new();
    let mut widget = flow.first_child();
    while let Some(w) = widget {
        let next = w.next_sibling();
        if let Ok(child) = w.downcast::<FlowBoxChild>() {
            if let Some(item) = remote_child_item(&child) {
                let id = item.id.as_deref().unwrap_or("").trim();
                if item.tags.is_empty() && item.url.contains("wallhaven") && !id.is_empty() {
                    jobs.push(WallhavenEnrichJob {
                        id: id.to_string(),
                        child: SendWeakRef::from(child.downgrade()),
                    });
                }
            }
        }
        widget = next;
    }
    if jobs.is_empty() {
        return;
    }

    let name_w = SendWeakRef::from(preview_name.downgrade());
    let stats_w = stats.clone();

    let mut need_http = Vec::new();
    for job in jobs {
        if let Some(details) = catalog::cached_wallhaven_details(&job.id) {
            apply_wallhaven_details_to_tile(
                &job.child, &job.id, &details, &pending, &name_w, &stats_w,
            );
        } else {
            need_http.push(job);
        }
    }
    if need_http.is_empty() {
        return;
    }

    if let Some(sel) = flow.selected_children().into_iter().next() {
        if let Some(item) = remote_child_item(&sel) {
            if let Some(id) = item.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(pos) = need_http.iter().position(|j| j.id == id) {
                    let job = need_http.remove(pos);
                    need_http.insert(0, job);
                }
            }
        }
    }

    let (tx, rx) = std::sync::mpsc::channel::<WallhavenEnrichJob>();
    let rx = Arc::new(Mutex::new(rx));
    let workers = catalog::WALLHAVEN_DETAIL_HTTP_MAX
        .min(need_http.len())
        .max(1);
    for _ in 0..workers {
        let rx = Arc::clone(&rx);
        let api_key = api_key.to_string();
        let gen = Arc::clone(&gen);
        let pending = Arc::clone(&pending);
        let name_w = name_w.clone();
        let stats_w = stats_w.clone();
        std::thread::spawn(move || loop {
            let job = rx.lock().unwrap().recv();
            let Ok(job) = job else {
                break;
            };
            let Ok(details) = catalog::fetch_wallhaven_details(&job.id, &api_key) else {
                continue;
            };
            let child_w = job.child;
            let id = job.id;
            let gen = Arc::clone(&gen);
            let pending = Arc::clone(&pending);
            let name_w = name_w.clone();
            let stats_w = stats_w.clone();
            glib::idle_add_once(move || {
                if gen.load(Ordering::Relaxed) != my {
                    return;
                }
                apply_wallhaven_details_to_tile(
                    &child_w, &id, &details, &pending, &name_w, &stats_w,
                );
            });
        });
    }
    for job in need_http {
        let _ = tx.send(job);
    }
}

pub(crate) fn apply_wallhaven_details_to_tile(
    child_w: &SendWeakRef<FlowBoxChild>,
    id: &str,
    details: &catalog::WallhavenDetails,
    pending: &Arc<Mutex<Option<catalog::RemoteItem>>>,
    name_w: &SendWeakRef<Label>,
    stats_w: &StatsPaneRefs,
) {
    let Some(child) = child_w.upgrade() else {
        return;
    };
    let Some(mut item) = remote_child_item(&child) else {
        return;
    };
    item.apply_wallhaven_details(details);
    unsafe {
        child.set_data("nwall-remote", item.clone());
    }
    if let Some(cap) = child_caption(&child) {
        cap.set_text(&item.tile_label());
    }
    child.set_tooltip_text(Some(&item.tooltip_label()));
    let selected = child.is_selected()
        || pending
            .lock()
            .ok()
            .and_then(|g| g.as_ref().and_then(|i| i.id.clone()))
            .as_deref()
            == Some(id);
    if selected {
        if let Ok(mut g) = pending.lock() {
            *g = Some(item.clone());
        }
        if let Some(name) = name_w.upgrade() {
            set_preview_title(&name, &item.tile_label());
        }
        stats_w.apply(&item.stats());
    }
}

pub(crate) fn apply_archive_details_to_tile(
    child_w: &SendWeakRef<FlowBoxChild>,
    id: &str,
    details: &catalog::ArchiveDetails,
    pending: &Arc<Mutex<Option<catalog::RemoteItem>>>,
    name_w: &SendWeakRef<Label>,
    stats_w: &StatsPaneRefs,
) {
    let Some(child) = child_w.upgrade() else {
        return;
    };
    let Some(mut item) = remote_child_item(&child) else {
        return;
    };
    item.apply_archive_details(details);
    unsafe {
        child.set_data("nwall-remote", item.clone());
    }
    if let Some(cap) = child_caption(&child) {
        cap.set_text(&item.tile_label());
    }
    child.set_tooltip_text(Some(&item.tooltip_label()));
    let selected = child.is_selected()
        || pending
            .lock()
            .ok()
            .and_then(|g| g.as_ref().and_then(|i| i.id.clone()))
            .as_deref()
            == Some(id);
    if selected {
        if let Ok(mut g) = pending.lock() {
            *g = Some(item.clone());
        }
        if let Some(name) = name_w.upgrade() {
            set_preview_title(&name, &item.tile_label());
        }
        stats_w.apply(&item.stats());
    }
}

pub(crate) fn apply_github_details_to_tile(
    child_w: &SendWeakRef<FlowBoxChild>,
    details: &catalog::GitHubDetails,
    pending: &Arc<Mutex<Option<catalog::RemoteItem>>>,
    name_w: &SendWeakRef<Label>,
    stats_w: &StatsPaneRefs,
) {
    let Some(child) = child_w.upgrade() else {
        return;
    };
    let Some(mut item) = remote_child_item(&child) else {
        return;
    };
    item.apply_github_details(details);
    unsafe {
        child.set_data("nwall-remote", item.clone());
    }
    if let Some(cap) = child_caption(&child) {
        cap.set_text(&item.tile_label());
    }
    child.set_tooltip_text(Some(&item.tooltip_label()));
    let selected = child.is_selected()
        || pending.lock().ok().and_then(|g| g.as_ref().map(|i| i.url.clone()))
            == Some(item.url.clone());
    if selected {
        if let Ok(mut g) = pending.lock() {
            *g = Some(item.clone());
        }
        if let Some(name) = name_w.upgrade() {
            set_preview_title(&name, &item.tile_label());
        }
        stats_w.apply(&item.stats());
    }
}

pub(crate) fn child_caption(child: &FlowBoxChild) -> Option<Label> {
    unsafe {
        child
            .data::<Label>("nwall-caption")
            .map(|p| p.as_ref().clone())
    }
}

