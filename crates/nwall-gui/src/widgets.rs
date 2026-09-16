use adw::prelude::*;
use gtk::{Align, Box as GtkBox, CheckButton, Label, Orientation, SpinButton};

pub(crate) fn section_label(text: &str) -> Label {
    let l = Label::new(Some(text));
    l.set_halign(Align::Start);
    l.add_css_class("title-4");
    l
}

pub(crate) fn filter_heading(text: &str) -> Label {
    let l = Label::new(Some(text));
    l.set_halign(Align::Start);
    l.add_css_class("heading");
    l
}

pub(crate) fn check_row(buttons: &[&CheckButton]) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    for b in buttons {
        row.append(*b);
    }
    row
}

pub(crate) fn filter_page_box() -> GtkBox {
    let b = GtkBox::new(Orientation::Vertical, 8);
    b.set_margin_start(10);
    b.set_margin_end(10);
    b.set_margin_top(8);
    b.set_margin_bottom(8);
    b
}

pub(crate) fn set_video_playback_rows_visible(
    fps: &GtkBox,
    mute: &GtkBox,
    volume: &GtkBox,
    video: bool,
) {
    fps.set_visible(video);
    mute.set_visible(video);
    volume.set_visible(video);
}

pub(crate) fn spin_with_percent(spin: &SpinButton) -> &SpinButton {
    spin_with_suffix(spin, "%")
}

pub(crate) fn spin_with_minutes(spin: &SpinButton) -> &SpinButton {
    spin_with_suffix(spin, "min")
}

pub(crate) fn spin_with_suffix<'a>(spin: &'a SpinButton, suffix: &str) -> &'a SpinButton {
    spin.set_halign(Align::End);
    // SpinButton: numeric(false) allows suffix in entry text.
    spin.set_numeric(false);
    let out_suffix = suffix.to_string();
    spin.connect_output(move |s| {
        s.set_text(&format!("{}{}", s.value().round() as i64, out_suffix));
        glib::Propagation::Stop
    });
    let in_suffix = suffix.to_string();
    spin.connect_input(move |s| {
        let t = s.text();
        let trimmed = t
            .trim()
            .strip_suffix(in_suffix.as_str())
            .unwrap_or(t.trim())
            .trim();
        if trimmed.is_empty() {
            return None;
        }
        match trimmed.parse::<f64>() {
            Ok(v) => Some(Ok(v)),
            Err(_) => Some(Err(())),
        }
    });
    let max_abs = spin
        .adjustment()
        .upper()
        .abs()
        .max(spin.adjustment().lower().abs());
    let chars = format!("{}{}", max_abs.round() as i64, suffix).len().max(2) as i32;
    spin.set_width_chars(chars);
    spin.set_max_width_chars(chars);
    spin.update();
    spin
}

pub(crate) fn labeled_row(text: &str, widget: &impl IsA<gtk::Widget>) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 12);
    row.add_css_class("labeled-row");
    row.set_hexpand(true);
    let l = Label::new(Some(text));
    l.set_halign(Align::Start);
    l.set_hexpand(true);
    row.append(&l);
    row.append(widget);
    row
}

pub(crate) fn preview_option_row(text: &str, widget: &impl IsA<gtk::Widget>) -> GtkBox {
    let row = labeled_row(text, widget);
    row.add_css_class("preview-option-row");
    row.set_spacing(8);
    row
}
