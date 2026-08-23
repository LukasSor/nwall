
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adw::prelude::*;
use glib::object::SendWeakRef;
use gtk::{
    gdk, gio, glib, Align, Box as GtkBox, FlowBox, FlowBoxChild, Label, LinkButton, Orientation,
    Picture, PolicyType, ScrolledWindow,
};
use nwall_catalog as catalog;
use nwall_ipc::{is_video};

use crate::consts::*;
use crate::ui::discover::remote_child_item;
use crate::ui::preview::{file_nonempty, set_preview_title};

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
        root.add_css_class("preview-stats");

        let content = GtkBox::new(Orientation::Vertical, 14);
        content.set_hexpand(true);
        content.set_valign(Align::Start);
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
    header.set_xalign(0.0);
    header.set_hexpand(false);
    header
}

pub(crate) fn stats_prop_label(text: &str, label_group: &gtk::SizeGroup) -> Label {
    let lbl = Label::new(Some(text));
    lbl.add_css_class("stats-prop-label");
    lbl.add_css_class("dim-label");
    lbl.set_halign(Align::Start);
    lbl.set_valign(Align::Center);
    lbl.set_xalign(0.0);
    lbl.set_hexpand(false);
    lbl.set_size_request(STATS_LABEL_W, -1);
    lbl.set_wrap(true);
    lbl.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    lbl.set_ellipsize(gtk::pango::EllipsizeMode::None);
    label_group.add_widget(&lbl);
    lbl
}

pub(crate) fn stats_section(title: &str) -> (GtkBox, GtkBox) {
    let section = GtkBox::new(Orientation::Vertical, 4);
    section.set_hexpand(true);
    section.add_css_class("stats-section");

    let rows = GtkBox::new(Orientation::Vertical, 4);
    rows.set_hexpand(true);
    rows.add_css_class("stats-section-rows");

    section.append(&stats_section_header(title));
    section.append(&rows);
    (section, rows)
}

pub(crate) fn stats_prop_row(label: &str, value: &str, label_group: &gtk::SizeGroup) -> GtkBox {
    stats_prop_row_inner(label, value, label_group)
}

pub(crate) fn stats_prop_row_wrapped(
    label: &str,
    value: &str,
    label_group: &gtk::SizeGroup,
) -> GtkBox {
    stats_prop_row(label, value, label_group)
}

pub(crate) fn stats_prop_row_inner(
    label: &str,
    value: &str,
    label_group: &gtk::SizeGroup,
) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 10);
    row.set_hexpand(true);
    row.set_halign(Align::Fill);
    row.set_valign(Align::Start);
    row.add_css_class("stats-prop-row");

    let val = Label::new(Some(value));
    val.add_css_class("stats-prop-value");
    val.set_halign(Align::Start);
    val.set_valign(Align::Start);
    val.set_xalign(0.0);
    val.set_hexpand(true);
    val.set_selectable(true);
    val.set_wrap(true);
    val.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    val.set_ellipsize(gtk::pango::EllipsizeMode::None);

    row.append(&{
        let lbl = stats_prop_label(label, label_group);
        lbl.set_valign(Align::Start);
        lbl
    });
    row.append(&val);
    row
}

pub(crate) fn stats_link_row(url: &str, label_group: &gtk::SizeGroup) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 10);
    row.set_hexpand(true);
    row.set_halign(Align::Fill);
    row.set_valign(Align::Start);
    row.add_css_class("stats-prop-row");

    let escaped = glib::markup_escape_text(url);
    let val = Label::new(None);
    val.set_markup(&format!("<a href=\"{escaped}\">Open page</a>"));
    val.add_css_class("stats-prop-value");
    val.add_css_class("preview-stats-link");
    val.set_halign(Align::Start);
    val.set_valign(Align::Start);
    val.set_xalign(0.0);
    val.set_hexpand(true);
    val.set_wrap(true);
    val.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    val.set_ellipsize(gtk::pango::EllipsizeMode::None);

    row.append(&{
        let lbl = stats_prop_label("Link", label_group);
        lbl.set_valign(Align::Start);
        lbl
    });
    row.append(&val);
    row
}

pub(crate) fn stats_uploader_row(
    label: &str,
    name: &str,
    avatar_url: Option<&str>,
    avatar_gen: &Arc<AtomicU64>,
    label_group: &gtk::SizeGroup,
) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 10);
    row.set_hexpand(true);
    row.set_halign(Align::Fill);
    row.set_valign(Align::Start);
    row.add_css_class("stats-prop-row");
    row.add_css_class("stats-uploader-row");

    let avatar = adw::Avatar::new(AVATAR_PX, Some(name), true);
    avatar.add_css_class("uploader-avatar");
    let has_url = avatar_url.map(str::trim).is_some_and(|s| !s.is_empty());
    if has_url {
        bind_avatar(&avatar, avatar_url, avatar_gen);
    } else {
        avatar.set_visible(true);
    }

    let name_l = Label::new(Some(name));
    name_l.add_css_class("stats-prop-value");
    name_l.set_halign(Align::Start);
    name_l.set_valign(Align::Start);
    name_l.set_xalign(0.0);
    name_l.set_hexpand(true);
    name_l.set_wrap(true);
    name_l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    name_l.set_ellipsize(gtk::pango::EllipsizeMode::None);
    name_l.set_selectable(true);

    let value = GtkBox::new(Orientation::Horizontal, 8);
    value.set_halign(Align::Start);
    value.set_hexpand(true);
    value.set_valign(Align::Start);
    value.append(&avatar);
    value.append(&name_l);

    row.append(&{
        let lbl = stats_prop_label(label, label_group);
        lbl.set_valign(Align::Start);
        lbl
    });
    row.append(&value);
    row
}

pub(crate) fn stats_colors_block(hexes: &[&str]) -> GtkBox {
    let section = GtkBox::new(Orientation::Vertical, 4);
    section.set_hexpand(true);
    section.add_css_class("stats-section");
    section.add_css_class("stats-colors");

    let swatches = GtkBox::new(Orientation::Horizontal, 4);
    swatches.set_halign(Align::Start);
    swatches.set_hexpand(true);
    swatches.add_css_class("color-swatches");
    swatches.add_css_class("stats-block-body");
    for hex in hexes {
        swatches.append(&color_swatch(hex));
    }

    section.append(&stats_section_header("Colors"));
    section.append(&swatches);
    section
}

pub(crate) fn stats_tags_block(tags: &[String]) -> GtkBox {
    let section = GtkBox::new(Orientation::Vertical, 4);
    section.set_halign(Align::Fill);
    section.set_hexpand(true);
    section.add_css_class("stats-section");
    section.add_css_class("stats-tags");

    let header = stats_section_header("Tags");

    let tags_store = Label::new(Some(&tags.join("\n")));
    tags_store.set_visible(false);

    let tags_cloud = GtkBox::new(Orientation::Vertical, 4);
    tags_cloud.set_halign(Align::Fill);
    tags_cloud.set_hexpand(true);
    tags_cloud.set_valign(Align::Start);
    tags_cloud.set_vexpand(false);
    tags_cloud.add_css_class("preview-tags-cloud");
    tags_cloud.add_css_class("stats-block-body");

    let last_tag_w = Rc::new(Cell::new(0i32));
    {
        let store = tags_store.clone();
        let last_w = Rc::clone(&last_tag_w);
        tags_cloud.add_tick_callback(move |cloud, _clock| {
            let width = cloud.width();
            if width < 32 || (last_w.get() - width).abs() <= 2 {
                return glib::ControlFlow::Continue;
            }
            last_w.set(width);
            reflow_tag_cloud(cloud, &tags_from_store(&store), width);
            glib::ControlFlow::Continue
        });
    }

    let w = tags_cloud.width();
    if w >= 32 {
        reflow_tag_cloud(&tags_cloud, tags, w);
    }

    section.append(&header);
    section.append(&tags_store);
    section.append(&tags_cloud);
    section
}

pub(crate) fn append_prop(
    rows: &GtkBox,
    label: &str,
    value: Option<String>,
    label_group: &gtk::SizeGroup,
) -> bool {
    let Some(v) = value.filter(|s| !s.trim().is_empty()) else {
        return false;
    };
    rows.append(&stats_prop_row(label, v.trim(), label_group));
    true
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

    let label_group = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);

    let (media_sec, media_rows) = stats_section("Media");
    let mut media_n = 0usize;
    let resolution = match (stats.width, stats.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some(format!("{w}×{h}")),
        _ if pending => Some("…".into()),
        _ => None,
    };
    if append_prop(&media_rows, "Resolution", resolution, &label_group) {
        media_n += 1;
    }
    let size = stats
        .file_size
        .filter(|n| *n > 0)
        .map(catalog::human_bytes)
        .or_else(|| pending.then(|| "…".into()));
    if append_prop(&media_rows, "Size", size, &label_group) {
        media_n += 1;
    }
    let duration = stats
        .duration_secs
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(catalog::format_duration);
    if append_prop(&media_rows, "Duration", duration, &label_group) {
        media_n += 1;
    }
    if let Some(fps) = stats.fps.filter(|f| *f > 0.0) {
        if append_prop(
            &media_rows,
            "FPS",
            Some(stats_fps_display(fps)),
            &label_group,
        ) {
            media_n += 1;
        }
    }
    if let Some(t) = nonempty_gui(&stats.file_type) {
        if append_prop(
            &media_rows,
            "Type",
            Some(stats_short_type(t)),
            &label_group,
        ) {
            media_n += 1;
        }
    }
    if media_n > 0 {
        content.append(&media_sec);
    }

    let (source_sec, source_rows) = stats_section("Source");
    let mut source_n = 0usize;
    let source = stats_source_name(stats);
    if append_prop(&source_rows, "Source", source.clone(), &label_group) {
        source_n += 1;
    }
    if let Some(t) = nonempty_gui(&stats.title) {
        let skip = catalog::tag_caption(&stats.tags, 8).as_deref() == Some(t);
        if !skip && append_prop(&source_rows, "Title", Some(t.to_string()), &label_group)
        {
            source_n += 1;
        }
    }
    if append_prop(
        &source_rows,
        "Credit",
        stats_credit_display(stats, source.as_deref()),
        &label_group,
    ) {
        source_n += 1;
    }
    if let Some(r) = nonempty_gui(&stats.repo) {
        if append_prop(&source_rows, "Repo", Some(r.to_string()), &label_group) {
            source_n += 1;
        }
    }
    if nonempty_gui(&stats.repo).is_some() {
        if let Some(id) = nonempty_gui(&stats.source_id) {
            if append_prop(&source_rows, "Path", Some(id.to_string()), &label_group) {
                source_n += 1;
            }
        }
    }
    if let Some(u) = nonempty_gui(&stats.page_url) {
        source_rows.append(&stats_link_row(u, &label_group));
        source_n += 1;
    }
    if source_n > 0 {
        content.append(&source_sec);
    }

    let (details_sec, details_rows) = stats_section("Details");
    let mut details_n = 0usize;
    if let Some(c) = nonempty_gui(&stats.category) {
        if append_prop(
            &details_rows,
            "Category",
            Some(catalog::title_case_ascii(c)),
            &label_group,
        ) {
            details_n += 1;
        }
    }
    if let Some(p) = nonempty_gui(&stats.purity) {
        if append_prop(
            &details_rows,
            "Purity",
            Some(p.to_ascii_uppercase()),
            &label_group,
        ) {
            details_n += 1;
        }
    }
    if let Some(n) = stats.views {
        if append_prop(&details_rows, "Views", Some(n.to_string()), &label_group) {
            details_n += 1;
        }
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
        if append_prop(
            &details_rows,
            fav_label,
            Some(n.to_string()),
            &label_group,
        ) {
            details_n += 1;
        }
    }
    if let Some(n) = stats.downloads.filter(|n| *n > 0) {
        if append_prop(
            &details_rows,
            "Downloads",
            Some(n.to_string()),
            &label_group,
        ) {
            details_n += 1;
        }
    }
    if let Some(n) = stats.comments.filter(|n| *n > 0) {
        if append_prop(
            &details_rows,
            "Comments",
            Some(n.to_string()),
            &label_group,
        ) {
            details_n += 1;
        }
    }
    if let Some(d) = nonempty_gui(&stats.date) {
        if append_prop(&details_rows, "Date", Some(d.to_string()), &label_group) {
            details_n += 1;
        }
    }
    if let Some(c) = nonempty_gui(&stats.collection) {
        if append_prop(
            &details_rows,
            "Collection",
            Some(c.to_string()),
            &label_group,
        ) {
            details_n += 1;
        }
    }
    if let Some(l) = nonempty_gui(&stats.license) {
        if append_prop(
            &details_rows,
            "License",
            Some(l.to_string()),
            &label_group,
        ) {
            details_n += 1;
        }
    }
    if let Some(d) = nonempty_gui(&stats.description) {
        details_rows.append(&stats_prop_row_wrapped(
            "Description",
            d,
            &label_group,
        ));
        details_n += 1;
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
        details_rows.append(&stats_uploader_row(
            label,
            name,
            stats.avatar.as_deref(),
            avatar_gen,
            &label_group,
        ));
        details_n += 1;
    }
    if details_n > 0 {
        content.append(&details_sec);
    }

    let colors: Vec<&str> = stats
        .colors
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .take(8)
        .collect();
    if !colors.is_empty() {
        content.append(&stats_colors_block(&colors));
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
        content.append(&stats_tags_block(&tags));
    }

    content.set_visible(true);
}

pub(crate) fn nonempty_gui(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

pub(crate) fn tags_from_store(store: &Label) -> Vec<String> {
    let t = store.text();
    if t.is_empty() {
        Vec::new()
    } else {
        t.split('\n')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
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
    chip.set_halign(Align::Center);
    chip.set_valign(Align::Center);
    chip.set_hexpand(false);
    chip.set_vexpand(false);
    chip.set_overflow(gtk::Overflow::Hidden);

    let l = Label::new(Some(t));
    l.add_css_class("preview-tag-label");
    l.set_halign(Align::Center);
    l.set_valign(Align::Center);
    l.set_hexpand(false);
    chip.append(&l);

    apply_local_css(
        &chip,
        &format!(
            "box.preview-tag {{\n\
             background-color: rgba({br},{bg},{bb},0.62);\n\
             border-radius: 999px;\n\
             padding: 2px 9px;\n\
             border: none;\n\
            }}\n\
             box.preview-tag label {{\n\
             color: rgba({fr},{fg},{fb},0.88);\n\
             font-size: 0.82em;\n\
             font-weight: 500;\n\
            }}"
        ),
    );
    chip
}

pub(crate) fn new_tag_row() -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, TAG_ROW_GAP);
    row.set_halign(Align::Start);
    row.set_hexpand(true);
    row.set_valign(Align::Center);
    row.add_css_class("preview-tags-row");
    row
}

pub(crate) fn reflow_tag_cloud(cloud: &GtkBox, tags: &[String], width: i32) {
    while let Some(ch) = cloud.first_child() {
        cloud.remove(&ch);
    }
    if tags.is_empty() {
        return;
    }
    let inner = width.max(1);
    let mut row = new_tag_row();
    let mut row_w = 0;
    let mut rows = 1usize;
    for t in tags {
        let chip = tag_chip(t);
        let tw = chip.measure(Orientation::Horizontal, -1).1.max(8);
        let need = if row_w == 0 {
            tw
        } else {
            row_w + TAG_ROW_GAP + tw
        };
        if row_w > 0 && need > inner && rows < TAG_MAX_ROWS {
            cloud.append(&row);
            row = new_tag_row();
            row_w = 0;
            rows += 1;
        } else if row_w > 0 && need > inner && rows >= TAG_MAX_ROWS {
            break;
        }
        if row_w == 0 {
            row_w = tw;
        } else {
            row_w += TAG_ROW_GAP + tw;
        }
        row.append(&chip);
    }
    if row.first_child().is_some() {
        cloud.append(&row);
    }
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
        apply_local_css(
            &swatch,
            &format!(
                "box.color-swatch {{\n\
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

pub(crate) fn apply_local_css(widget: &impl IsA<gtk::Widget>, css: &str) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(css);
    widget
        .style_context()
        .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
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

