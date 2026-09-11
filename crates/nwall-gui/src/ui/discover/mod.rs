
mod filters;
pub(crate) mod thumbs;

pub(crate) use filters::build_discover_filters;
pub(crate) use thumbs::{
    bind_remote_image_thumb_prio, bind_remote_video_still,
    bump_remote_thumb_gen, make_fixed_thumb, pack_tile, refresh_remote_tile_still,
    refresh_remote_tile_still_if_missing, refresh_remote_tile_texture_if_missing,
    remote_still_path, remote_tile_needs_still,
};
pub(crate) use crate::ui::preview::{bind_thumb, file_nonempty};

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adw::prelude::*;
use glib::object::SendWeakRef;
use gtk::prelude::EditableExt;
use gtk::{
    gdk, glib, Align, Box as GtkBox, Button, DropDown, Entry, EventControllerFocus,
    EventControllerKey, FlowBox, FlowBoxChild, InputPurpose, Label, Orientation, Paned, ScrolledWindow, Spinner, Stack, StringList,
};
use nwall_catalog as catalog;
use nwall_ipc::{
    default_config_path, is_video, CatalogSource,
    Config,
};

use crate::app::{
    replace_string_list, visible_catalog_sources, watch_sources, SourceWatchers,
};
use crate::ipc_util::apply_wallpaper;
use crate::ui::gallery::{compact_flow, gallery_dirs, kind_badge, refresh_gallery};
use crate::ui::preview::{
    arm_preview_loading, bind_preview_host_size, fill_sidebar_button, fit_preview_still, hide_preview_loading,
    make_fixed_preview, make_preview_split, new_preview_sidebar,
    preview_caption_label, preview_section_label, preview_sidebar_head, preview_stats_scroll,
    preview_title, reset_preview_aspect, set_preview_title, start_live_preview,
    stop_live_preview, PreviewSession,
};
use crate::ui::stats::{
    apply_archive_details_to_tile, apply_github_details_to_tile,
    apply_wallhaven_details_to_tile, enrich_wallhaven_tiles, make_stats_pane, schedule_local_stats,
};

use self::filters::persist_discover_filters;
use self::thumbs::extract_remote_still;

const DISCOVER_PAGE_JUMP_MAX: u32 = 99_999;

fn discover_page_ceiling(last_page: &AtomicU32) -> u32 {
    let last = last_page.load(Ordering::Relaxed);
    if last > 0 {
        last
    } else {
        DISCOVER_PAGE_JUMP_MAX
    }
}

fn jump_discover_page(entry: &Entry, page: &AtomicU32, last_page: &AtomicU32, run: &dyn Fn()) {
    let max = discover_page_ceiling(last_page);
    let cur = page.load(Ordering::Relaxed);
    let mut p = match entry.text().trim().parse::<u32>() {
        Ok(v) => v,
        Err(_) => {
            entry.set_text(&cur.to_string());
            return;
        }
    };
    p = p.clamp(1, max);
    entry.set_text(&p.to_string());
    if p != cur {
        page.store(p, Ordering::Relaxed);
        run();
    }
}

fn nudge_discover_page(entry: &Entry, page: &AtomicU32, last_page: &AtomicU32, run: &dyn Fn(), delta: i32) {
    let max = discover_page_ceiling(last_page);
    let cur = page.load(Ordering::Relaxed);
    let next = (cur as i64 + delta as i64).clamp(1, max as i64) as u32;
    entry.set_text(&next.to_string());
    if next != cur {
        page.store(next, Ordering::Relaxed);
        run();
    }
}

pub(crate) fn update_discover_pager(
    pager: &GtkBox,
    prev: &Button,
    next: &Button,
    page_entry: &Entry,
    page_of: &Label,
    kind: &str,
    page: u32,
    last: Option<u32>,
    n_items: usize,
) {
    let paging = catalog::supports_paging(kind);
    pager.set_visible(paging);
    if !paging {
        prev.set_sensitive(false);
        next.set_sensitive(false);
        page_entry.set_sensitive(false);
        page_entry.set_text("1");
        page_of.set_text("");
        return;
    }
    page_entry.set_sensitive(true);
    prev.set_sensitive(page > 1);
    let at_last = last.map(|m| page >= m).unwrap_or(false);
    next.set_sensitive(!at_last && n_items > 0);
    let max = last.map(|m| m.max(1)).unwrap_or(DISCOVER_PAGE_JUMP_MAX);
    let page = page.clamp(1, max);
    page_entry.set_text(&page.to_string());
    match last {
        Some(m) => page_of.set_text(&format!("/ {m}")),
        None => page_of.set_text(""),
    }
}

fn show_discover_center_status(body: &Stack, spin: &Spinner, lbl: &Label, text: &str) {
    spin.stop();
    spin.set_visible(false);
    lbl.remove_css_class("title-3");
    lbl.add_css_class("discover-status");
    lbl.add_css_class("dim-label");
    lbl.set_wrap(true);
    lbl.set_justify(gtk::Justification::Center);
    lbl.set_text(text);
    body.set_visible_child_name("loading");
}

fn begin_discover_loading(body: &Stack, spin: &Spinner, lbl: &Label, text: &str) {
    lbl.remove_css_class("discover-status");
    lbl.remove_css_class("dim-label");
    lbl.add_css_class("title-3");
    lbl.set_wrap(true);
    lbl.set_justify(gtk::Justification::Center);
    lbl.set_text(text);
    spin.set_visible(true);
    spin.start();
    body.set_visible_child_name("loading");
}

fn discover_fetch_status(err: &str) -> (bool, String) {
    let lower = err.to_ascii_lowercase();
    let key_err = lower.contains("api key")
        || lower.contains("needs an api key")
        || (lower.contains("add a") && lower.contains("pat") && lower.contains("settings"));
    if lower.contains("rate limit") || lower.contains("rate-limited") {
        let msg = if lower.contains("settings") || lower.contains("api key") || lower.contains("pat")
        {
            if lower.contains("github") {
                "Rate limit reached — add a GitHub PAT in Settings, or try again later".into()
            } else if lower.contains("nasa") {
                "Rate limit reached — add an API key in Settings, or try again later".into()
            } else {
                "Rate limit reached — add an API key in Settings, or try again later".into()
            }
        } else {
            "Rate limit reached — try again later".into()
        };
        return (false, msg);
    }
    if lower.contains("no results")
        || lower.contains("no images")
        || lower.contains("no wallpapers")
        || lower.contains("not found")
    {
        return (false, "No results found".into());
    }
    if key_err {
        return (true, err.to_string());
    }
    (false, format!("Failed: {err}"))
}

pub(crate) fn build_discover(
    cfg: &Config,
    selected: Arc<Mutex<Option<PathBuf>>>,
    monitors: Rc<RefCell<Vec<String>>>,
    gallery_body: Stack,
    gallery_flow: FlowBox,
    _gallery_dirs: Vec<PathBuf>,
    gallery_filter: DropDown,
    gallery_search: Entry,
    live: Arc<Mutex<Option<PreviewSession>>>,
    preview_gen: Arc<AtomicU64>,
    source_watchers: SourceWatchers,
    on_applied: Arc<dyn Fn() + Send + Sync>,
    sidebar_pct: f64,
    show_stats: bool,
) -> (GtkBox, Paned, ScrolledWindow, Rc<dyn Fn()>) {
    let root = GtkBox::new(Orientation::Vertical, 0);
    let bar = GtkBox::new(Orientation::Horizontal, 8);
    bar.set_margin_top(10);
    bar.set_margin_start(12);
    bar.set_margin_end(12);

    let listed: Rc<RefCell<Vec<CatalogSource>>> =
        Rc::new(RefCell::new(catalog::selectable_sources(&cfg.sources)));
    let names: Vec<String> = listed.borrow().iter().map(|s| s.name.clone()).collect();
    let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let list = StringList::new(&name_refs);
    let source_dd = DropDown::new(Some(list.clone()), Option::<&gtk::Expression>::None);
    let wallhaven_idx = names
        .iter()
        .position(|n| n.eq_ignore_ascii_case("Wallhaven"))
        .unwrap_or(0);
    source_dd.set_selected(wallhaven_idx as u32);
    let suppress_src = Rc::new(Cell::new(false));
    let search = Entry::new();
    search.set_hexpand(true);
    search.set_placeholder_text(Some("Search, or leave empty"));
    let filters = build_discover_filters();
    if let Ok(cfg) = Config::load(&default_config_path()) {
        filters.apply_saved(&cfg.discover_filters);
    }
    filters.sync_nsfw_visibility(&listed.borrow());
    {
        let list = list.clone();
        let listed = Rc::clone(&listed);
        let dd = source_dd.clone();
        let suppress = Rc::clone(&suppress_src);
        let filters = filters.clone();
        watch_sources(&source_watchers, move || {
            let next = visible_catalog_sources();
            let prev_name = listed
                .borrow()
                .get(dd.selected() as usize)
                .map(|s| s.name.clone());
            let unchanged = listed.borrow().len() == next.len()
                && listed
                    .borrow()
                    .iter()
                    .zip(next.iter())
                    .all(|(a, b)| a.name == b.name && a.kind == b.kind);
            if unchanged {
                filters.sync_nsfw_visibility(&next);
                *listed.borrow_mut() = next;
                return;
            }
            suppress.set(true);
            let names: Vec<String> = next.iter().map(|s| s.name.clone()).collect();
            replace_string_list(&list, &names);
            let sel = prev_name
                .and_then(|p| next.iter().position(|s| s.name == p))
                .or_else(|| {
                    next.iter()
                        .position(|s| s.name.eq_ignore_ascii_case("Wallhaven"))
                })
                .unwrap_or(0);
            filters.sync_nsfw_visibility(&next);
            *listed.borrow_mut() = next;
            if !names.is_empty() {
                dd.set_selected(sel as u32);
            }
            suppress.set(false);
        });
    }
    let go_spin = Spinner::new();
    go_spin.set_visible(false);
    let go_lbl = Label::new(Some("Browse"));
    let go_inner = GtkBox::new(Orientation::Horizontal, 8);
    go_inner.set_halign(Align::Center);
    go_inner.append(&go_spin);
    go_inner.append(&go_lbl);
    let go = Button::new();
    go.set_child(Some(&go_inner));
    go.add_css_class("suggested-action");
    bar.append(&Label::new(Some("Source:")));
    bar.append(&source_dd);
    bar.append(&search);
    bar.append(&filters.button);
    bar.append(&go);

    let flow = FlowBox::new();
    compact_flow(&flow);
    let scroll = ScrolledWindow::builder()
        .child(&flow)
        .vexpand(true)
        .hexpand(true)
        .build();

    let load_spin = Spinner::new();
    load_spin.set_size_request(42, 42);
    load_spin.set_halign(Align::Center);
    let load_lbl = Label::new(Some("Loading…"));
    load_lbl.add_css_class("title-3");
    let loading = GtkBox::new(Orientation::Vertical, 12);
    loading.set_hexpand(true);
    loading.set_vexpand(true);
    loading.set_valign(Align::Center);
    loading.set_halign(Align::Center);
    loading.add_css_class("discover-loading");
    loading.append(&load_spin);
    loading.append(&load_lbl);

    let err_lbl = Label::new(None);
    err_lbl.set_wrap(true);
    err_lbl.set_justify(gtk::Justification::Center);
    err_lbl.set_halign(Align::Center);
    err_lbl.set_valign(Align::Center);
    err_lbl.set_hexpand(true);
    err_lbl.set_vexpand(true);
    err_lbl.add_css_class("discover-error");

    let body = Stack::new();
    body.set_vexpand(true);
    body.set_hexpand(true);
    body.add_named(&scroll, Some("grid"));
    body.add_named(&loading, Some("loading"));
    body.add_named(&err_lbl, Some("error"));
    body.set_visible_child_name("loading");

    let (preview_host, preview_pic, preview_loading) = make_fixed_preview();
    let preview_name = preview_title("Click a wallpaper");
    let preview_meta = preview_caption_label("Preview only", false);

    let download_btn = Button::with_label("Download");
    download_btn.set_sensitive(false);
    fill_sidebar_button(&download_btn);
    let download_apply_btn = Button::with_label("Download & Apply");
    download_apply_btn.add_css_class("suggested-action");
    download_apply_btn.set_sensitive(false);
    fill_sidebar_button(&download_apply_btn);

    let preview_stats = make_stats_pane();
    let preview_box = new_preview_sidebar();
    let head = preview_sidebar_head();
    head.append(&preview_section_label());
    head.append(&preview_host);
    head.append(&preview_name);
    head.append(&preview_meta);
    head.append(&download_btn);
    head.append(&download_apply_btn);
    preview_box.append(&head);
    let discover_stats_scroll = preview_stats_scroll(&preview_stats.root);
    discover_stats_scroll.set_visible(show_stats);
    preview_box.append(&discover_stats_scroll);

    bind_preview_host_size(&preview_box, &preview_host);
    let split = make_preview_split(&body, &preview_box, sidebar_pct);

    let status = Label::new(None);
    status.set_halign(Align::Start);
    status.set_margin_start(12);
    status.set_margin_end(12);
    status.set_margin_bottom(8);
    status.add_css_class("dim-label");
    status.set_ellipsize(gtk::pango::EllipsizeMode::Middle);

    let pager = GtkBox::new(Orientation::Horizontal, 8);
    pager.set_halign(Align::Center);
    pager.set_margin_top(6);
    pager.set_margin_bottom(2);
    let prev_btn = Button::with_label("Prev");
    let page_cap = Label::new(Some("Page"));
    page_cap.add_css_class("dim-label");
    let page_entry = Entry::new();
    page_entry.add_css_class("discover-page-entry");
    page_entry.set_input_purpose(InputPurpose::Digits);
    page_entry.set_max_length(5);
    page_entry.set_width_chars(4);
    page_entry.set_max_width_chars(5);
    EditableExt::set_alignment(&page_entry, 0.5);
    page_entry.set_text("1");
    page_entry.set_tooltip_text(Some("Type a page number and press Enter (↑/↓ to step)"));
    let page_of = Label::new(None);
    page_of.add_css_class("dim-label");
    let next_btn = Button::with_label("Next");
    prev_btn.set_sensitive(false);
    pager.append(&prev_btn);
    pager.append(&page_cap);
    pager.append(&page_entry);
    pager.append(&page_of);
    pager.append(&next_btn);

    root.append(&bar);
    root.append(&split);
    root.append(&pager);
    root.append(&status);

    let page = Arc::new(AtomicU32::new(1));
    let last_page = Arc::new(AtomicU32::new(0));
    let initial_src = listed.borrow().get(wallhaven_idx).cloned();
    let initial_kind = initial_src
        .as_ref()
        .map(|s| s.kind.clone())
        .unwrap_or_default();
    filters.show_for_kind(&initial_kind);
    pager.set_visible(catalog::supports_paging(&initial_kind));

    let load_gen = Arc::new(AtomicU64::new(0));
    let pending: Arc<Mutex<Option<catalog::RemoteItem>>> = Arc::new(Mutex::new(None));
    let run = Rc::new({
        let flow = flow.clone();
        let source_dd = source_dd.clone();
        let listed = Rc::clone(&listed);
        let search = search.clone();
        let filters = filters.clone();
        let status = status.clone();
        let body = body.clone();
        let err_lbl = err_lbl.clone();
        let load_spin = load_spin.clone();
        let load_lbl = load_lbl.clone();
        let go = go.clone();
        let go_spin = go_spin.clone();
        let go_lbl = go_lbl.clone();
        let pager = pager.clone();
        let prev_btn = prev_btn.clone();
        let next_btn = next_btn.clone();
        let page_entry = page_entry.clone();
        let page_of = page_of.clone();
        let page = Arc::clone(&page);
        let last_page = Arc::clone(&last_page);
        let load_gen = Arc::clone(&load_gen);
        let live = Arc::clone(&live);
        let pending = Arc::clone(&pending);
        let preview_name = preview_name.clone();
        let preview_stats = preview_stats.clone();
        move || {
            stop_live_preview(&live);
            *pending.lock().unwrap() = None;
            preview_stats.clear();
            set_preview_title(&preview_name, "Click a wallpaper");
            let idx = source_dd.selected() as usize;
            let Some(src) = listed.borrow().get(idx).cloned() else {
                show_discover_center_status(
                    &body,
                    &load_spin,
                    &load_lbl,
                    "No source selected — add one in Settings",
                );
                status.set_text("No source selected — add one in Settings");
                return;
            };
            let opts = filters.collect(search.text().to_string(), page.load(Ordering::Relaxed), &src.kind);
            let my = load_gen.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = bump_remote_thumb_gen();
            let api_key = catalog::source_api_key(&src);
            begin_discover_loading(&body, &load_spin, &load_lbl, &format!("Loading {}…", src.name));
            go.set_sensitive(false);
            prev_btn.set_sensitive(false);
            next_btn.set_sensitive(false);
            go_spin.set_visible(true);
            go_spin.start();
            go_lbl.set_text("Loading…");
            status.set_text(&format!("Loading {}…", src.name));
            let flow_w = SendWeakRef::from(flow.downgrade());
            let status_w = SendWeakRef::from(status.downgrade());
            let body_w = SendWeakRef::from(body.downgrade());
            let err_w = SendWeakRef::from(err_lbl.downgrade());
            let load_spin_w = SendWeakRef::from(load_spin.downgrade());
            let load_lbl_w = SendWeakRef::from(load_lbl.downgrade());
            let go_w = SendWeakRef::from(go.downgrade());
            let go_spin_w = SendWeakRef::from(go_spin.downgrade());
            let go_lbl_w = SendWeakRef::from(go_lbl.downgrade());
            let pager_w = SendWeakRef::from(pager.downgrade());
            let prev_w = SendWeakRef::from(prev_btn.downgrade());
            let next_w = SendWeakRef::from(next_btn.downgrade());
            let page_entry_w = SendWeakRef::from(page_entry.downgrade());
            let page_of_w = SendWeakRef::from(page_of.downgrade());
            let gen = Arc::clone(&load_gen);
            let page_a = Arc::clone(&page);
            let last_a = Arc::clone(&last_page);
            let kind = src.kind.clone();
            let pending_a = Arc::clone(&pending);
            let name_w = SendWeakRef::from(preview_name.downgrade());
            let stats_refs = preview_stats.refs();
            std::thread::spawn(move || {
                let result = catalog::fetch_source(&src, &opts);
                let query = opts.query.clone();
                glib::idle_add_once(move || {
                    if gen.load(Ordering::Relaxed) != my {
                        return;
                    }
                    if let Some(sp) = load_spin_w.upgrade() {
                        sp.stop();
                    }
                    if let Some(b) = go_w.upgrade() {
                        b.set_sensitive(true);
                    }
                    if let Some(sp) = go_spin_w.upgrade() {
                        sp.stop();
                        sp.set_visible(false);
                    }
                    if let Some(l) = go_lbl_w.upgrade() {
                        l.set_text("Browse");
                    }
                    let Some(flow) = flow_w.upgrade() else {
                        return;
                    };
                    while let Some(c) = flow.first_child() {
                        flow.remove(&c);
                    }
                    let (n_items, pg, last) = match &result {
                        Ok(fetched) => (fetched.items.len(), fetched.page, fetched.last_page),
                        Err(_) => (0, page_a.load(Ordering::Relaxed), None),
                    };
                    page_a.store(pg, Ordering::Relaxed);
                    last_a.store(last.unwrap_or(0), Ordering::Relaxed);
                    if let (Some(p), Some(prev), Some(next), Some(entry), Some(of)) = (
                        pager_w.upgrade(),
                        prev_w.upgrade(),
                        next_w.upgrade(),
                        page_entry_w.upgrade(),
                        page_of_w.upgrade(),
                    ) {
                        update_discover_pager(&p, &prev, &next, &entry, &of, &kind, pg, last, n_items);
                    }
                    match result {
                        Ok(fetched) if fetched.items.is_empty() => {
                            if let (Some(b), Some(sp), Some(l)) = (
                                body_w.upgrade(),
                                load_spin_w.upgrade(),
                                load_lbl_w.upgrade(),
                            ) {
                                show_discover_center_status(
                                    &b,
                                    &sp,
                                    &l,
                                    "No results found",
                                );
                            }
                            if let Some(s) = status_w.upgrade() {
                                s.set_text("No results found");
                            }
                        }
                        Ok(fetched) => {
                            if let Some(s) = status_w.upgrade() {
                                s.set_text(&format!("{} items", fetched.items.len()));
                            }
                            for (i, mut it) in fetched.items.into_iter().enumerate() {
                                apply_discover_caption(&mut it, &query);
                                flow.insert(&make_remote_tile_at(&it, i), -1);
                            }
                            if let Some(name) = name_w.upgrade() {
                                enrich_wallhaven_tiles(
                                    &flow,
                                    &api_key,
                                    Arc::clone(&gen),
                                    my,
                                    pending_a,
                                    &name,
                                    &stats_refs,
                                );
                            }
                            if let Some(b) = body_w.upgrade() {
                                b.set_visible_child_name("grid");
                            }
                        }
                        Err(e) => {
                            let msg = format!("{e:#}");
                            let (key_err, hint) = discover_fetch_status(&msg);
                            if key_err {
                                if let Some(l) = err_w.upgrade() {
                                    l.set_text(&hint);
                                }
                                if let Some(b) = body_w.upgrade() {
                                    b.set_visible_child_name("error");
                                }
                            } else if let (Some(b), Some(sp), Some(l)) = (
                                body_w.upgrade(),
                                load_spin_w.upgrade(),
                                load_lbl_w.upgrade(),
                            ) {
                                show_discover_center_status(&b, &sp, &l, &hint);
                            }
                            if let Some(s) = status_w.upgrade() {
                                s.set_text(&hint);
                            }
                        }
                    }
                });
            });
        }
    });
    let run_c = Rc::clone(&run);
    go.connect_clicked(move |_| run_c());
    search.connect_activate({
        let run = Rc::clone(&run);
        let page = Arc::clone(&page);
        move |_| {
            page.store(1, Ordering::Relaxed);
            run();
        }
    });
    let reload_filters = {
        let run = Rc::clone(&run);
        let page = Arc::clone(&page);
        let filters = filters.clone();
        Rc::new(move || {
            persist_discover_filters(&filters);
            page.store(1, Ordering::Relaxed);
            run();
        }) as Rc<dyn Fn()>
    };
    if let Some(ref s) = initial_src {
        filters.sync_github_repos(s, &reload_filters);
    }
    source_dd.connect_selected_notify({
        let run = Rc::clone(&run);
        let suppress = Rc::clone(&suppress_src);
        let listed = Rc::clone(&listed);
        let filters = filters.clone();
        let pager = pager.clone();
        let prev_btn = prev_btn.clone();
        let next_btn = next_btn.clone();
        let page_entry = page_entry.clone();
        let page_of = page_of.clone();
        let page = Arc::clone(&page);
        let last_page = Arc::clone(&last_page);
        let reload_repos = Rc::clone(&reload_filters);
        move |dd| {
            if suppress.get() {
                return;
            }
            let src = listed.borrow().get(dd.selected() as usize).cloned();
            let kind = src
                .as_ref()
                .map(|s| s.kind.clone())
                .unwrap_or_default();
            filters.sync_nsfw_visibility(&listed.borrow());
            filters.show_for_kind(&kind);
            if let Some(ref s) = src {
                filters.sync_github_repos(s, &reload_repos);
            } else {
                filters.github_repos_box.set_visible(false);
            }
            page.store(1, Ordering::Relaxed);
            last_page.store(0, Ordering::Relaxed);
            update_discover_pager(&pager, &prev_btn, &next_btn, &page_entry, &page_of, &kind, 1, None, 1);
            run();
        }
    });
    {
        let r = Rc::clone(&reload_filters);
        filters.sfw.connect_toggled(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.sketchy.connect_toggled(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.nsfw.connect_toggled(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.cat_general.connect_toggled(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.cat_anime.connect_toggled(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.cat_people.connect_toggled(move |_| r());
        let r = Rc::clone(&reload_filters);
        let filters_sort = filters.clone();
        filters.sort.connect_selected_notify(move |_| {
            filters_sort.sync_toplist_range();
            r();
        });
        let r = Rc::clone(&reload_filters);
        filters.top_range.connect_selected_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.atleast.connect_selected_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.ratios.connect_selected_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.hide_ai.connect_active_notify(move |_| r());
        let filters_full = filters.clone();
        filters.load_full_preview.connect_active_notify(move |_| {
            persist_discover_filters(&filters_full);
        });
        let r = Rc::clone(&reload_filters);
        filters.safesearch.connect_active_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.pixabay_order.connect_selected_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters
            .pixabay_category
            .connect_selected_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters
            .pixabay_video_type
            .connect_selected_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        let filters_px = filters.clone();
        filters.pixabay_media.connect_selected_notify(move |_| {
            filters_px.sync_pixabay_media_filters();
            r();
        });
        let r = Rc::clone(&reload_filters);
        filters.coverr_sort.connect_selected_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.archive_order.connect_selected_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.archive_media.connect_selected_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        filters.github_media.connect_selected_notify(move |_| r());
        let r = Rc::clone(&reload_filters);
        let filters_gh = filters.clone();
        filters.github_repos_all.connect_toggled(move |all| {
            if filters_gh.github_repos_suppress.get() {
                return;
            }
            filters_gh.github_repos_suppress.set(true);
            let on = all.is_active();
            for (_, cb) in filters_gh.github_repo_checks.borrow().iter() {
                cb.set_active(on);
            }
            filters_gh.github_repos_suppress.set(false);
            persist_discover_filters(&filters_gh);
            r();
        });
    }
    prev_btn.connect_clicked({
        let run = Rc::clone(&run);
        let page = Arc::clone(&page);
        move |_| {
            let p = page.load(Ordering::Relaxed);
            if p > 1 {
                page.store(p - 1, Ordering::Relaxed);
                run();
            }
        }
    });
    next_btn.connect_clicked({
        let run = Rc::clone(&run);
        let page = Arc::clone(&page);
        let last_page = Arc::clone(&last_page);
        move |_| {
            let p = page.load(Ordering::Relaxed);
            let last = last_page.load(Ordering::Relaxed);
            if last > 0 && p >= last {
                return;
            }
            page.store(p.saturating_add(1), Ordering::Relaxed);
            run();
        }
    });
    page_entry.connect_activate({
        let run = Rc::clone(&run);
        let page = Arc::clone(&page);
        let last_page = Arc::clone(&last_page);
        move |entry| {
            jump_discover_page(entry, &page, &last_page, run.as_ref());
        }
    });
    {
        let keys = EventControllerKey::new();
        let run = Rc::clone(&run);
        let page = Arc::clone(&page);
        let last_page = Arc::clone(&last_page);
        let entry = page_entry.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Up || key == gdk::Key::KP_Up {
                nudge_discover_page(&entry, &page, &last_page, run.as_ref(), 1);
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::Down || key == gdk::Key::KP_Down {
                nudge_discover_page(&entry, &page, &last_page, run.as_ref(), -1);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        page_entry.add_controller(keys);
    }
    {
        let focus = EventControllerFocus::new();
        let page = Arc::clone(&page);
        let entry = page_entry.clone();
        focus.connect_leave(move |_| {
            entry.set_text(&page.load(Ordering::Relaxed).to_string());
        });
        page_entry.add_controller(focus);
    }

    let ensure_first_load = {
        let run = Rc::clone(&run);
        let load_gen = Arc::clone(&load_gen);
        let done = Cell::new(false);
        Rc::new(move || {
            if done.replace(true) || load_gen.load(Ordering::Relaxed) > 0 {
                return;
            }
            run();
        }) as Rc<dyn Fn()>
    };

    let pending_s = Arc::clone(&pending);
    let pic_s = preview_pic.clone();
    let host_s = preview_host.clone();
    let name_s = preview_name.clone();
    let meta_s = preview_meta.clone();
    let loading_s = preview_loading.clone();
    let stats_s = preview_stats.clone();
    let dl_s = download_btn.clone();
    let dla_s = download_apply_btn.clone();
    let status_s = status.clone();
    let live_s = Arc::clone(&live);
    let gen_s = Arc::clone(&preview_gen);
    let stats_gen = Arc::new(AtomicU64::new(0));
    let stats_gen_s = Arc::clone(&stats_gen);
    let listed_s = Rc::clone(&listed);
    let source_dd_s = source_dd.clone();
    flow.connect_selected_children_changed(move |flow| {
        let Some(child) = flow.selected_children().into_iter().next() else {
            return;
        };
        let Some(item) = remote_child_item(&child) else {
            return;
        };
        *pending_s.lock().unwrap() = Some(item.clone());
        dl_s.set_sensitive(true);
        dla_s.set_sensitive(true);
        set_preview_title(&name_s, &item.tile_label());
        meta_s.set_text(if item.video {
            "Playing preview video"
        } else {
            "Showing preview image"
        });
        let my_stats = stats_gen_s.fetch_add(1, Ordering::Relaxed) + 1;
        stats_s.apply(&item.stats());
        if item.tags.is_empty() && item.url.contains("wallhaven") {
            if let Some(id) = item.id.clone() {
                let api_key = listed_s
                    .borrow()
                    .get(source_dd_s.selected() as usize)
                    .map(catalog::source_api_key)
                    .unwrap_or_default();
                let child_w = SendWeakRef::from(child.downgrade());
                let pending = Arc::clone(&pending_s);
                let name_w = SendWeakRef::from(name_s.downgrade());
                let stats_w = stats_s.refs();
                let gen = Arc::clone(&stats_gen_s);
                std::thread::spawn(move || {
                    let Ok(details) = catalog::fetch_wallhaven_details(&id, &api_key) else {
                        return;
                    };
                    glib::idle_add_once(move || {
                        if gen.load(Ordering::Relaxed) != my_stats {
                            return;
                        }
                        apply_wallhaven_details_to_tile(
                            &child_w, &id, &details, &pending, &name_w, &stats_w,
                        );
                    });
                });
            }
        } else if item.url.contains("archive.org") {
            if let Some(id) = item.id.clone() {
                let child_w = SendWeakRef::from(child.downgrade());
                let pending = Arc::clone(&pending_s);
                let name_w = SendWeakRef::from(name_s.downgrade());
                let stats_w = stats_s.refs();
                let gen = Arc::clone(&stats_gen_s);
                std::thread::spawn(move || {
                    let Ok(details) = catalog::fetch_archive_details(&id) else {
                        return;
                    };
                    glib::idle_add_once(move || {
                        if gen.load(Ordering::Relaxed) != my_stats {
                            return;
                        }
                        apply_archive_details_to_tile(
                            &child_w, &id, &details, &pending, &name_w, &stats_w,
                        );
                    });
                });
            }
        } else if item.repo.is_some() {
            let child_w = SendWeakRef::from(child.downgrade());
            let pending = Arc::clone(&pending_s);
            let name_w = SendWeakRef::from(name_s.downgrade());
            let stats_w = stats_s.refs();
            let gen = Arc::clone(&stats_gen_s);
            let item_g = item.clone();
            let api_key = listed_s
                .borrow()
                .get(source_dd_s.selected() as usize)
                .map(catalog::source_api_key)
                .unwrap_or_default();
            std::thread::spawn(move || {
                let Ok(details) = catalog::fetch_github_details(&item_g, &api_key) else {
                    return;
                };
                glib::idle_add_once(move || {
                    if gen.load(Ordering::Relaxed) != my_stats {
                        return;
                    }
                    apply_github_details_to_tile(
                        &child_w, &details, &pending, &name_w, &stats_w,
                    );
                });
            });
        }
        stop_live_preview(&live_s);
        let my = gen_s.fetch_add(1, Ordering::SeqCst) + 1;
        pic_s.set_paintable(Option::<&gdk::Paintable>::None);
        reset_preview_aspect(&host_s);
        crate::ui::preview::ensure_preview_contain(&pic_s);
        loading_s.set_busy(false);
        status_s.set_text("Preview loaded — Download or Download & Apply");
        if item.video {
            let tile_child_w = SendWeakRef::from(child.downgrade());
            let cfg = Config::load(&default_config_path()).unwrap_or_default();
            let fast_video = cfg.fast_video_preview;
            let mut showed_still = false;
            if let Some(th) = &item.thumb {
                let cached = catalog::cached_path(th, "thumb");
                if file_nonempty(&cached) {
                    fit_preview_still(&pic_s, &cached);
                    showed_still = true;
                    refresh_remote_tile_still_if_missing(&child, &cached);
                }
            } else {
                let dest = remote_still_path(&item.url);
                if file_nonempty(&dest) {
                    fit_preview_still(&pic_s, &dest);
                    showed_still = true;
                    refresh_remote_tile_still_if_missing(&child, &dest);
                }
            }
            if fast_video {
                meta_s.set_text("Fast preview");
                if showed_still {
                } else if let Some(th) = item.thumb.clone() {
                    arm_preview_loading(&loading_s, &gen_s, my);
                    let pic_w = SendWeakRef::from(pic_s.downgrade());
                    let tile_w = tile_child_w.clone();
                    let layer_w = SendWeakRef::from(loading_s.layer.downgrade());
                    let spinner_w = SendWeakRef::from(loading_s.spinner.downgrade());
                    let gen = Arc::clone(&gen_s);
                    std::thread::spawn(move || {
                        let result = catalog::download(&th, "thumb");
                        glib::idle_add_once(move || {
                            if gen.load(Ordering::Relaxed) != my {
                                return;
                            }
                            if let Ok(path) = result {
                                if let Some(pic) = pic_w.upgrade() {
                                    fit_preview_still(&pic, &path);
                                }
                                if let Some(c) = tile_w.upgrade() {
                                    refresh_remote_tile_still_if_missing(&c, &path);
                                }
                            }
                            hide_preview_loading(&layer_w, &spinner_w);
                        });
                    });
                } else {
                    arm_preview_loading(&loading_s, &gen_s, my);
                    let url = item.url.clone();
                    let dest = remote_still_path(&url);
                    let pic_w = SendWeakRef::from(pic_s.downgrade());
                    let tile_w = tile_child_w.clone();
                    let layer_w = SendWeakRef::from(loading_s.layer.downgrade());
                    let spinner_w = SendWeakRef::from(loading_s.spinner.downgrade());
                    let gen = Arc::clone(&gen_s);
                    std::thread::spawn(move || {
                        if !file_nonempty(&dest) {
                            let _ = extract_remote_still(&url, &dest);
                        }
                        glib::idle_add_once(move || {
                            if gen.load(Ordering::Relaxed) != my {
                                return;
                            }
                            if file_nonempty(&dest) {
                                if let Some(pic) = pic_w.upgrade() {
                                    fit_preview_still(&pic, &dest);
                                }
                                if let Some(c) = tile_w.upgrade() {
                                    refresh_remote_tile_still_if_missing(&c, &dest);
                                }
                            }
                            hide_preview_loading(&layer_w, &spinner_w);
                        });
                    });
                }
            } else {
                meta_s.set_text("Playing preview video");
                arm_preview_loading(&loading_s, &gen_s, my);
                let still_path = remote_still_path(&item.url);
                let still_out = if remote_tile_needs_still(&child) {
                    let tile_for_still = tile_child_w.clone();
                    Some(crate::ui::preview::LivePreviewStillOut {
                        path: still_path.clone(),
                        on_first_frame: Box::new(move |tex| {
                            if let Some(c) = tile_for_still.upgrade() {
                                refresh_remote_tile_texture_if_missing(&c, tex);
                            }
                        }),
                    })
                } else {
                    None
                };
                let frames = start_live_preview(
                    item.url.clone(),
                    &pic_s,
                    &live_s,
                    &gen_s,
                    my,
                    &loading_s,
                    still_out,
                );
                if !showed_still {
                    let pic_w = SendWeakRef::from(pic_s.downgrade());
                    let gen = Arc::clone(&gen_s);
                    if let Some(th) = item.thumb.clone() {
                        let tile_w = tile_child_w.clone();
                        std::thread::spawn(move || {
                            let Ok(path) = catalog::download(&th, "thumb") else {
                                return;
                            };
                            let path_tile = path.clone();
                            glib::idle_add_once(move || {
                                if gen.load(Ordering::Relaxed) == my
                                    && frames.load(Ordering::Relaxed) == 0
                                {
                                    if let Some(pic) = pic_w.upgrade() {
                                        fit_preview_still(&pic, &path);
                                    }
                                }
                                if let Some(c) = tile_w.upgrade() {
                                    refresh_remote_tile_still(&c, &path_tile);
                                }
                            });
                        });
                    } else {
                        let url = item.url.clone();
                        let dest = still_path;
                        let tile_w = tile_child_w.clone();
                        std::thread::spawn(move || {
                            let deadline = Instant::now() + Duration::from_secs(14);
                            while Instant::now() < deadline {
                                if file_nonempty(&dest) {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(80));
                            }
                            if !file_nonempty(&dest) {
                                let _ = extract_remote_still(&url, &dest);
                            }
                            if !file_nonempty(&dest) {
                                return;
                            }
                            glib::idle_add_once(move || {
                                if gen.load(Ordering::Relaxed) == my
                                    && frames.load(Ordering::Relaxed) == 0
                                {
                                    if let Some(pic) = pic_w.upgrade() {
                                        fit_preview_still(&pic, &dest);
                                    }
                                }
                                if let Some(c) = tile_w.upgrade() {
                                    refresh_remote_tile_still_if_missing(&c, &dest);
                                }
                            });
                        });
                    }
                }
            }
        } else {
            let cfg = Config::load(&default_config_path()).unwrap_or_default();
            let is_wallhaven = item.url.contains("wallhaven")
                || item.credit.as_deref() == Some("Wallhaven");
            let want_full = !cfg.fast_image_preview
                || (is_wallhaven && cfg.discover_filters.wallhaven.load_full_preview);

            if want_full {
                meta_s.set_text("Loading full preview…");
                let url = item.url.clone();
                let cached = catalog::cached_path(&url, "preview");
                let tile_child_w = SendWeakRef::from(child.downgrade());
                if file_nonempty(&cached) {
                    fit_preview_still(&pic_s, &cached);
                    refresh_remote_tile_still(&child, &cached);
                    meta_s.set_text("Showing full preview");
                } else {
                    if let Some(th) = item.thumb.clone() {
                        let thumb_cached = catalog::cached_path(&th, "thumb");
                        if file_nonempty(&thumb_cached) {
                            fit_preview_still(&pic_s, &thumb_cached);
                        }
                    }
                    arm_preview_loading(&loading_s, &gen_s, my);
                    let pic_w = SendWeakRef::from(pic_s.downgrade());
                    let layer_w = SendWeakRef::from(loading_s.layer.downgrade());
                    let spinner_w = SendWeakRef::from(loading_s.spinner.downgrade());
                    let meta_w = SendWeakRef::from(meta_s.downgrade());
                    let gen = Arc::clone(&gen_s);
                    std::thread::spawn(move || {
                        let result = catalog::download(&url, "preview");
                        glib::idle_add_once(move || {
                            if gen.load(Ordering::Relaxed) != my {
                                return;
                            }
                            if let Ok(path) = result {
                                if let Some(pic) = pic_w.upgrade() {
                                    fit_preview_still(&pic, &path);
                                }
                                if let Some(c) = tile_child_w.upgrade() {
                                    refresh_remote_tile_still(&c, &path);
                                }
                                if let Some(m) = meta_w.upgrade() {
                                    m.set_text("Showing full preview");
                                }
                            }
                            hide_preview_loading(&layer_w, &spinner_w);
                        });
                    });
                }
            } else {
                meta_s.set_text("Fast preview");
                let url = item.thumb.clone().unwrap_or_else(|| item.url.clone());
                let cached = catalog::cached_path(&url, "thumb");
                if file_nonempty(&cached) {
                    fit_preview_still(&pic_s, &cached);
                } else {
                    let pic_w = SendWeakRef::from(pic_s.downgrade());
                    let gen = Arc::clone(&gen_s);
                    std::thread::spawn(move || {
                        let Ok(path) = catalog::download(&url, "thumb") else {
                            return;
                        };
                        glib::idle_add_once(move || {
                            if gen.load(Ordering::Relaxed) != my {
                                return;
                            }
                            if let Some(pic) = pic_w.upgrade() {
                                fit_preview_still(&pic, &path);
                            }
                        });
                    });
                }
            }
        }
    });

    let start_save = {
        let pending = Arc::clone(&pending);
        let selected = Arc::clone(&selected);
        let monitors = Rc::clone(&monitors);
        let status = status.clone();
        let download_btn = download_btn.clone();
        let download_apply_btn = download_apply_btn.clone();
        let gallery_body_w = SendWeakRef::from(gallery_body.downgrade());
        let gallery_flow_w = SendWeakRef::from(gallery_flow.downgrade());
        let gallery_filter_w = SendWeakRef::from(gallery_filter.downgrade());
        let gallery_search_w = SendWeakRef::from(gallery_search.downgrade());
        let pic_w0 = SendWeakRef::from(preview_pic.downgrade());
        let host_w0 = SendWeakRef::from(preview_host.downgrade());
        let name_w0 = SendWeakRef::from(preview_name.downgrade());
        let stats_w0 = preview_stats.refs();
        let stats_gen = Arc::clone(&stats_gen);
        let on_applied = Arc::clone(&on_applied);
        let listed_dl = Rc::clone(&listed);
        let source_dd_dl = source_dd.clone();
        let live = Arc::clone(&live);
        Rc::new(move |apply_after: bool| {
            let Some(item) = pending.lock().unwrap().clone() else {
                status.set_text("Select a wallpaper first");
                return;
            };
            let outs = monitors.borrow().clone();
            if apply_after && outs.is_empty() {
                status.set_text("Select at least one monitor on the Wallpapers tab");
                return;
            }
            if apply_after {
                stop_live_preview(&live);
            }
            download_btn.set_sensitive(false);
            download_apply_btn.set_sensitive(false);
            status.set_text(&format!("Downloading {}…", item.tile_label()));
            let library = Config::load(&default_config_path())
                .unwrap_or_default()
                .library;
            let selected = Arc::clone(&selected);
            let status_w = SendWeakRef::from(status.downgrade());
            let dl_w = SendWeakRef::from(download_btn.downgrade());
            let dla_w = SendWeakRef::from(download_apply_btn.downgrade());
            let body_w = gallery_body_w.clone();
            let flow_w = gallery_flow_w.clone();
            let filter_w = gallery_filter_w.clone();
            let search_w = gallery_search_w.clone();
            let dirs = gallery_dirs(&library);
            let pic_w = pic_w0.clone();
            let host_w = host_w0.clone();
            let name_w = name_w0.clone();
            let stats_w = stats_w0.clone();
            let stats_gen = Arc::clone(&stats_gen);
            let on_applied = Arc::clone(&on_applied);
            let api_key = listed_dl
                .borrow()
                .get(source_dd_dl.selected() as usize)
                .map(catalog::source_api_key)
                .unwrap_or_default();
            let pending = Arc::clone(&pending);
            let live_apply = Arc::clone(&live);
            std::thread::spawn(move || {
                let mut item = item;
                catalog::enrich_item_for_save(&mut item, &api_key);
                let hint = item.library_file_stem();
                let result = catalog::download_to_library(
                    &item.url,
                    &library,
                    &hint,
                    item.id.as_deref(),
                );
                glib::idle_add_once(move || {
                    let restore = || {
                        if let Some(b) = dl_w.upgrade() {
                            b.set_sensitive(true);
                        }
                        if let Some(b) = dla_w.upgrade() {
                            b.set_sensitive(true);
                        }
                    };
                    match result {
                        Ok(path) => {
                            catalog::persist_remote_meta(&item, &path);
                            *selected.lock().unwrap() = Some(path.clone());
                            *pending.lock().unwrap() = Some(item.clone());
                            if let Some(name) = name_w.upgrade() {
                                set_preview_title(&name, &item.tile_label());
                            }
                            if let Some(pic) = pic_w.upgrade() {
                                if let Some(host) = host_w.upgrade() {
                                    reset_preview_aspect(&host);
                                }
                                crate::ui::preview::ensure_preview_contain(&pic);
                                if is_video(&path) {
                                    bind_thumb(&pic, path.clone(), true);
                                } else {
                                    fit_preview_still(&pic, &path);
                                }
                            }
                            {
                                let my = stats_gen.fetch_add(1, Ordering::Relaxed) + 1;
                                schedule_local_stats(
                                    path.clone(),
                                    Some(item.stats()),
                                    &stats_w,
                                    &stats_gen,
                                    my,
                                );
                            }
                            if let (Some(body), Some(flow), Some(filter), Some(search)) = (
                                body_w.upgrade(),
                                flow_w.upgrade(),
                                filter_w.upgrade(),
                                search_w.upgrade(),
                            ) {
                                refresh_gallery(
                                    &body,
                                    &flow,
                                    &dirs,
                                    filter.selected(),
                                    &search.text(),
                                );
                            }
                            if apply_after {
                                stop_live_preview(&live_apply);
                                match apply_wallpaper(&path, &outs) {
                                    Ok(()) => {
                                        if let Some(s) = status_w.upgrade() {
                                            s.set_text(&format!(
                                                "Applied to {}: {}",
                                                outs.join(", "),
                                                path.display()
                                            ));
                                        }
                                        on_applied();
                                    }
                                    Err(e) => {
                                        if let Some(s) = status_w.upgrade() {
                                            s.set_text(&format!(
                                                "Saved {}, apply failed: {e:#}",
                                                path.display()
                                            ));
                                        }
                                    }
                                }
                            } else if let Some(s) = status_w.upgrade() {
                                s.set_text(&format!("Saved {}", catalog::library_caption(&path)));
                            }
                        }
                        Err(e) => {
                            if let Some(s) = status_w.upgrade() {
                                s.set_text(&format!("Download failed: {e:#}"));
                            }
                        }
                    }
                    restore();
                });
            });
        })
    };
    let save_only = Rc::clone(&start_save);
    download_btn.connect_clicked(move |_| save_only(false));
    let save_apply = Rc::clone(&start_save);
    download_apply_btn.connect_clicked(move |_| save_apply(true));

    (root, split, discover_stats_scroll, ensure_first_load)
}

pub(crate) fn apply_discover_caption(item: &mut catalog::RemoteItem, query: &str) {
    if !item.url.contains("wallhaven") {
        return;
    }
    let q = query.trim();
    if q.is_empty() {
        return;
    }
    let n = item.name.trim();
    let leftover = n.is_empty()
        || n.contains('×')
        || (n.starts_with('#') && n.len() <= 9 && n[1..].chars().all(|c| c.is_ascii_alphanumeric()))
        || matches!(
            n.to_ascii_lowercase().as_str(),
            "general" | "anime" | "people"
        );
    if leftover {
        item.name = q.to_string();
    }
}

pub(crate) fn make_remote_tile_at(item: &catalog::RemoteItem, index: usize) -> FlowBoxChild {
    let priority = index < 24;
    let (host, pic) = make_fixed_thumb();
    let needs_video_still = item.video && item.thumb.is_none();
    if let Some(th) = &item.thumb {
        bind_remote_image_thumb_prio(&pic, th, priority);
    } else if item.video {
        host.add_css_class("thumb-placeholder");
    } else {
        bind_remote_image_thumb_prio(&pic, &item.url, priority);
    }
    host.add_overlay(&kind_badge(item.video));
    let child = pack_tile(&host, &item.tile_label());
    child.set_tooltip_text(Some(&item.tooltip_label()));
    unsafe {
        child.set_data("nwall-remote", item.clone());
        child.set_data("nwall-tile-pic", pic);
        child.set_data("nwall-tile-host", host);
    }
    if needs_video_still {
        bind_remote_video_still(&child, &item.url, priority);
    }
    child
}

pub(crate) fn remote_child_item(child: &FlowBoxChild) -> Option<catalog::RemoteItem> {
    unsafe {
        child
            .data::<catalog::RemoteItem>("nwall-remote")
            .map(|p| (*p.as_ref()).clone())
    }
}

