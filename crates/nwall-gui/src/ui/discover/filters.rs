use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{
    Align, Box as GtkBox, CheckButton, DropDown, GestureClick, Label, MenuButton, Orientation,
    PolicyType, Popover, ScrolledWindow, Stack, StringList, Switch,
};
use nwall_catalog as catalog;
use nwall_ipc::{
    default_config_path, discover_filters, CatalogSource, Config, DiscoverFiltersState,
};

use crate::consts::*;
use crate::widgets::{check_row, filter_heading, filter_page_box, labeled_row};

#[derive(Clone)]
pub(crate) struct DiscoverFilters {
    pub(crate) button: MenuButton,
    pub(crate) stack: Stack,
    pub(crate) purity_box: GtkBox,
    pub(crate) sfw: CheckButton,
    pub(crate) sketchy: CheckButton,
    pub(crate) nsfw: CheckButton,
    pub(crate) cat_general: CheckButton,
    pub(crate) cat_anime: CheckButton,
    pub(crate) cat_people: CheckButton,
    pub(crate) sort: DropDown,
    pub(crate) top_range_row: GtkBox,
    pub(crate) top_range: DropDown,
    pub(crate) atleast: DropDown,
    pub(crate) ratios: DropDown,
    pub(crate) hide_ai: Switch,
    pub(crate) load_full_preview: Switch,
    pub(crate) safesearch: Switch,
    pub(crate) pixabay_order: DropDown,
    pub(crate) pixabay_category: DropDown,
    pub(crate) pixabay_video_type: DropDown,
    pub(crate) pixabay_video_type_row: GtkBox,
    pub(crate) pixabay_media: DropDown,
    pub(crate) coverr_sort: DropDown,
    pub(crate) archive_order: DropDown,
    pub(crate) archive_media: DropDown,
    pub(crate) github_media: DropDown,
    pub(crate) github_repos_box: GtkBox,
    pub(crate) github_repos_list: GtkBox,
    pub(crate) github_repos_all: CheckButton,
    pub(crate) github_repo_checks: Rc<RefCell<Vec<(String, CheckButton)>>>,
    pub(crate) github_repos_suppress: Rc<Cell<bool>>,
}

impl DiscoverFilters {
    pub(crate) fn collect(&self, query: String, page: u32, kind: &str) -> catalog::SearchOpts {
        let purity_on = self.purity_box.get_visible();
        catalog::SearchOpts {
            query,
            page,
            purity_sfw: !purity_on || self.sfw.is_active(),
            purity_sketchy: purity_on && self.sketchy.is_active(),
            purity_nsfw: purity_on && self.nsfw.is_active(),
            cat_general: self.cat_general.is_active(),
            cat_anime: self.cat_anime.is_active(),
            cat_people: self.cat_people.is_active(),
            sorting: wallhaven_sort_key(self.sort.selected()),
            top_range: wallhaven_top_range_key(self.top_range.selected()),
            atleast: wallhaven_atleast_key(self.atleast.selected()),
            ratios: wallhaven_ratios_key(self.ratios.selected()),
            hide_ai: self.hide_ai.is_active(),
            safesearch: self.safesearch.is_active(),
            order: match kind {
                "coverr" => coverr_sort_key(self.coverr_sort.selected()),
                "archive" => archive_order_key(self.archive_order.selected()),
                _ => pixabay_order_key(self.pixabay_order.selected()),
            },
            category: pixabay_category_key(self.pixabay_category.selected()),
            video_type: pixabay_video_type_key(self.pixabay_video_type.selected()),
            seed: String::new(),
            media: match kind {
                "github" => github_media_key(self.github_media.selected()),
                "archive" => github_media_key(self.archive_media.selected()),
                "pixabay" => github_media_key(self.pixabay_media.selected()),
                _ => "all".into(),
            },
            github_repos: if kind == "github" {
                self.selected_github_repos()
            } else {
                Vec::new()
            },
        }
    }

    pub(crate) fn selected_github_repos(&self) -> Vec<String> {
        let checks = self.github_repo_checks.borrow();
        if checks.is_empty() {
            return Vec::new();
        }
        let selected: Vec<String> = checks
            .iter()
            .filter(|(_, c)| c.is_active())
            .map(|(n, _)| n.clone())
            .collect();
        if selected.len() == checks.len() {
            Vec::new()
        } else if selected.is_empty() {
            vec![GITHUB_REPO_FILTER_NONE.to_string()]
        } else {
            selected
        }
    }

    pub(crate) fn show_for_kind(&self, kind: &str) {
        self.stack
            .set_visible_child_name(discover_filter_page(kind));
        self.sync_toplist_range();
        self.sync_pixabay_media_filters();
        let has = discover_kind_has_filters(kind);
        self.button.set_sensitive(has);
        self.button.set_tooltip_text(Some(if has {
            "Search filters"
        } else {
            "No filters for this source"
        }));
        if kind != "github" {
            self.github_repos_box.set_visible(false);
        }
    }

    pub(crate) fn sync_toplist_range(&self) {
        let toplist = wallhaven_sort_key(self.sort.selected()) == "toplist";
        self.top_range_row.set_visible(toplist);
    }

    pub(crate) fn sync_pixabay_media_filters(&self) {
        let images_only = self.pixabay_media.selected() == 1;
        self.pixabay_video_type_row.set_visible(!images_only);
    }

    pub(crate) fn sync_nsfw_visibility(&self, sources: &[CatalogSource]) {
        let available = wallhaven_key_available(sources);
        self.purity_box.set_visible(available);
        if !available {
            if self.sketchy.is_active() {
                self.sketchy.set_active(false);
            }
            if self.nsfw.is_active() {
                self.nsfw.set_active(false);
            }
            if !self.sfw.is_active() {
                self.sfw.set_active(true);
            }
        }
    }

    pub(crate) fn sync_github_repos(&self, src: &CatalogSource, on_change: &Rc<dyn Fn()>) {
        if !src.kind.eq_ignore_ascii_case("github") {
            self.github_repos_box.set_visible(false);
            return;
        }
        let repos = catalog::github_unique_repos(src);
        self.github_repos_suppress.set(true);
        while let Some(child) = self.github_repos_list.first_child() {
            self.github_repos_list.remove(&child);
        }
        self.github_repo_checks.borrow_mut().clear();

        let saved = Config::load(&default_config_path())
            .map(|c| {
                if c.discover_filters.github.repos.is_empty() && !c.discover_github_repos.is_empty()
                {
                    c.discover_github_repos
                } else {
                    c.discover_filters.github.repos
                }
            })
            .unwrap_or_default();
        let multi = repos.len() > 1;
        self.github_repos_box.set_visible(multi);
        if !multi {
            self.github_repos_suppress.set(false);
            return;
        }

        let none_mode = saved.len() == 1 && saved[0] == GITHUB_REPO_FILTER_NONE;
        for repo in &repos {
            let cb = CheckButton::with_label(repo);
            let on = if none_mode {
                false
            } else {
                saved.is_empty() || saved.iter().any(|s| s.eq_ignore_ascii_case(repo))
            };
            cb.set_active(on);
            let r = Rc::clone(on_change);
            let filters = self.clone();
            cb.connect_toggled(move |_| {
                if filters.github_repos_suppress.get() {
                    return;
                }
                filters.sync_github_all_from_children();
                persist_discover_filters(&filters);
                r();
            });
            self.github_repos_list.append(&cb);
            self.github_repo_checks
                .borrow_mut()
                .push((repo.clone(), cb));
        }
        self.sync_github_all_from_children();
        self.github_repos_suppress.set(false);
    }

    pub(crate) fn selected_github_repos_for_persist(&self) -> Vec<String> {
        self.selected_github_repos()
    }

    pub(crate) fn sync_github_all_from_children(&self) {
        let checks = self.github_repo_checks.borrow();
        if checks.is_empty() {
            return;
        }
        let all_on = checks.iter().all(|(_, c)| c.is_active());
        self.github_repos_suppress.set(true);
        self.github_repos_all.set_active(all_on);
        self.github_repos_suppress.set(false);
    }

    pub(crate) fn apply_saved(&self, state: &DiscoverFiltersState) {
        let w = &state.wallhaven;
        self.sfw.set_active(w.sfw);
        self.sketchy.set_active(w.sketchy);
        self.nsfw.set_active(w.nsfw);
        self.cat_general.set_active(w.cat_general);
        self.cat_anime.set_active(w.cat_anime);
        self.cat_people.set_active(w.cat_people);
        self.sort.set_selected(w.sort);
        self.top_range.set_selected(w.top_range);
        self.atleast.set_selected(w.atleast);
        self.ratios.set_selected(w.ratios);
        self.hide_ai.set_active(w.hide_ai);
        self.load_full_preview.set_active(w.load_full_preview);

        let p = &state.pixabay;
        self.safesearch.set_active(p.safesearch);
        self.pixabay_order.set_selected(p.order);
        self.pixabay_media.set_selected(p.media);
        self.pixabay_video_type.set_selected(p.video_type);
        self.pixabay_category.set_selected(p.category);

        self.coverr_sort.set_selected(state.coverr.sort);

        let a = &state.archive;
        self.archive_order.set_selected(a.order);
        self.archive_media.set_selected(a.media);

        self.github_media.set_selected(state.github.media);

        self.sync_toplist_range();
        self.sync_pixabay_media_filters();
    }

    pub(crate) fn snapshot(&self) -> DiscoverFiltersState {
        DiscoverFiltersState {
            wallhaven: discover_filters::WallhavenFiltersState {
                sfw: self.sfw.is_active(),
                sketchy: self.sketchy.is_active(),
                nsfw: self.nsfw.is_active(),
                cat_general: self.cat_general.is_active(),
                cat_anime: self.cat_anime.is_active(),
                cat_people: self.cat_people.is_active(),
                sort: self.sort.selected(),
                top_range: self.top_range.selected(),
                atleast: self.atleast.selected(),
                ratios: self.ratios.selected(),
                hide_ai: self.hide_ai.is_active(),
                load_full_preview: self.load_full_preview.is_active(),
            },
            pixabay: discover_filters::PixabayFiltersState {
                safesearch: self.safesearch.is_active(),
                order: self.pixabay_order.selected(),
                media: self.pixabay_media.selected(),
                video_type: self.pixabay_video_type.selected(),
                category: self.pixabay_category.selected(),
            },
            coverr: discover_filters::CoverrFiltersState {
                sort: self.coverr_sort.selected(),
            },
            archive: discover_filters::ArchiveFiltersState {
                order: self.archive_order.selected(),
                media: self.archive_media.selected(),
            },
            github: discover_filters::GithubFiltersState {
                media: self.github_media.selected(),
                repos: self.selected_github_repos_for_persist(),
            },
        }
    }
}

pub(crate) fn persist_discover_filters(filters: &DiscoverFilters) {
    let mut cfg = Config::load(&default_config_path()).unwrap_or_default();
    cfg.discover_filters = filters.snapshot();
    cfg.discover_github_repos = cfg.discover_filters.github.repos.clone();
    let _ = cfg.save(&default_config_path());
}

pub(crate) fn wallhaven_key_available(sources: &[CatalogSource]) -> bool {
    sources
        .iter()
        .any(|s| s.kind == "wallhaven" && catalog::has_api_key(s))
}

pub(crate) fn discover_kind_has_filters(kind: &str) -> bool {
    matches!(
        kind,
        "wallhaven" | "pixabay" | "coverr" | "archive" | "github"
    )
}

pub(crate) fn discover_filter_page(kind: &str) -> &'static str {
    match kind {
        "wallhaven" => "wallhaven",
        "pixabay" => "pixabay",
        "coverr" => "coverr",
        "archive" => "archive",
        "github" => "github",
        _ => "none",
    }
}

pub(crate) fn wallhaven_sort_key(idx: u32) -> String {
    match idx {
        1 => "date_added",
        2 => "relevance",
        3 => "random",
        4 => "views",
        5 => "toplist",
        _ => "favorites",
    }
    .into()
}

pub(crate) fn wallhaven_top_range_key(idx: u32) -> String {
    match idx {
        0 => "1d",
        1 => "3d",
        2 => "1w",
        4 => "3M",
        5 => "6M",
        6 => "1y",
        _ => "1M",
    }
    .into()
}

pub(crate) fn wallhaven_atleast_key(idx: u32) -> String {
    match idx {
        0 => String::new(),
        2 => "2560x1440".into(),
        3 => "3840x2160".into(),
        _ => "1920x1080".into(),
    }
}

pub(crate) fn wallhaven_ratios_key(idx: u32) -> String {
    match idx {
        1 => "16x9".into(),
        2 => "16x10".into(),
        3 => "21x9".into(),
        4 => "32x9".into(),
        5 => "9x16".into(),
        _ => String::new(),
    }
}

pub(crate) fn pixabay_order_key(idx: u32) -> String {
    if idx == 1 {
        "latest".into()
    } else {
        "popular".into()
    }
}

pub(crate) fn pixabay_category_key(idx: u32) -> String {
    match idx {
        1 => "backgrounds",
        2 => "nature",
        3 => "places",
        4 => "travel",
        5 => "animals",
        6 => "people",
        7 => "buildings",
        8 => "computer",
        9 => "science",
        10 => "feelings",
        11 => "food",
        12 => "sports",
        13 => "music",
        _ => "",
    }
    .into()
}

pub(crate) fn pixabay_video_type_key(idx: u32) -> String {
    match idx {
        1 => "film",
        2 => "animation",
        _ => "all",
    }
    .into()
}

pub(crate) fn coverr_sort_key(idx: u32) -> String {
    match idx {
        1 => "date".into(),
        2 => "trending".into(),
        _ => "popular".into(),
    }
}

pub(crate) fn archive_order_key(idx: u32) -> String {
    match idx {
        1 => "popular".into(),
        2 => "latest".into(),
        _ => "relevance".into(),
    }
}

pub(crate) fn github_media_key(idx: u32) -> String {
    match idx {
        1 => "image".into(),
        2 => "video".into(),
        _ => "all".into(),
    }
}

pub(crate) fn build_discover_filters() -> DiscoverFilters {
    let stack = Stack::new();
    stack.set_vhomogeneous(false);

    let sfw = CheckButton::with_label("SFW");
    sfw.set_active(true);
    let sketchy = CheckButton::with_label("Sketchy");
    let nsfw = CheckButton::with_label("NSFW");
    let purity_heading = filter_heading("Purity");
    let purity_checks = check_row(&[&sfw, &sketchy, &nsfw]);
    let purity_box = GtkBox::new(Orientation::Vertical, 8);
    purity_box.append(&purity_heading);
    purity_box.append(&purity_checks);

    let cat_general = CheckButton::with_label("General");
    cat_general.set_active(true);
    let cat_anime = CheckButton::with_label("Anime");
    cat_anime.set_active(true);
    let cat_people = CheckButton::with_label("People");
    cat_people.set_active(true);
    let sort = DropDown::new(
        Some(StringList::new(&[
            "Favorites",
            "Latest",
            "Relevance",
            "Random",
            "Views",
            "Toplist",
        ])),
        Option::<&gtk::Expression>::None,
    );
    sort.set_selected(0);
    let top_range = DropDown::new(
        Some(StringList::new(&[
            "1 day", "3 days", "1 week", "1 month", "3 months", "6 months", "1 year",
        ])),
        Option::<&gtk::Expression>::None,
    );
    top_range.set_selected(3);
    let top_range_row = labeled_row("Toplist", &top_range);
    top_range_row.set_visible(false);
    let atleast = DropDown::new(
        Some(StringList::new(&["Any", "1080p", "1440p", "4K"])),
        Option::<&gtk::Expression>::None,
    );
    atleast.set_selected(0);
    let ratios = DropDown::new(
        Some(StringList::new(&[
            "Any", "16:9", "16:10", "21:9", "32:9", "9:16",
        ])),
        Option::<&gtk::Expression>::None,
    );
    ratios.set_selected(0);
    let hide_ai = Switch::new();
    hide_ai.set_active(false);
    hide_ai.set_valign(Align::Center);

    let load_full_preview = Switch::new();
    load_full_preview.set_active(true);
    load_full_preview.set_valign(Align::Center);
    load_full_preview.set_tooltip_text(Some(
        "Download the full wallpaper for the sidebar preview (real aspect). Overrides Settings → Fast image preview for Wallhaven.",
    ));

    let wh = filter_page_box();
    wh.append(&purity_box);
    wh.append(&filter_heading("Categories"));
    wh.append(&check_row(&[&cat_general, &cat_anime, &cat_people]));
    wh.append(&labeled_row("Sort", &sort));
    wh.append(&top_range_row);
    wh.append(&labeled_row("Min resolution", &atleast));
    wh.append(&labeled_row("Ratio", &ratios));
    wh.append(&labeled_row("Hide AI", &hide_ai));
    wh.append(&labeled_row("Full preview", &load_full_preview));
    stack.add_named(&wh, Some("wallhaven"));

    let safesearch = Switch::new();
    safesearch.set_active(true);
    safesearch.set_valign(Align::Center);
    let pixabay_order = DropDown::new(
        Some(StringList::new(&["Popular", "Latest"])),
        Option::<&gtk::Expression>::None,
    );
    pixabay_order.set_selected(0);
    let pixabay_media = DropDown::new(
        Some(StringList::new(&["All", "Images", "Videos"])),
        Option::<&gtk::Expression>::None,
    );
    pixabay_media.set_selected(0);
    let pixabay_video_type = DropDown::new(
        Some(StringList::new(&["All", "Film", "Animation"])),
        Option::<&gtk::Expression>::None,
    );
    pixabay_video_type.set_selected(0);
    let pixabay_video_type_row = labeled_row("Video type", &pixabay_video_type);
    let pixabay_category = DropDown::new(
        Some(StringList::new(&[
            "Any",
            "Backgrounds",
            "Nature",
            "Places",
            "Travel",
            "Animals",
            "People",
            "Buildings",
            "Computer",
            "Science",
            "Feelings",
            "Food",
            "Sports",
            "Music",
        ])),
        Option::<&gtk::Expression>::None,
    );
    pixabay_category.set_selected(0);
    let px = filter_page_box();
    px.append(&labeled_row("Safe search", &safesearch));
    px.append(&labeled_row("Order", &pixabay_order));
    px.append(&labeled_row("Media", &pixabay_media));
    px.append(&pixabay_video_type_row);
    px.append(&labeled_row("Category", &pixabay_category));
    stack.add_named(&px, Some("pixabay"));

    let coverr_sort = DropDown::new(
        Some(StringList::new(&["Popular", "Latest", "Trending"])),
        Option::<&gtk::Expression>::None,
    );
    coverr_sort.set_selected(0);
    let cv = filter_page_box();
    cv.append(&labeled_row("Sort", &coverr_sort));
    stack.add_named(&cv, Some("coverr"));

    let archive_order = DropDown::new(
        Some(StringList::new(&["Relevance", "Popular", "Latest"])),
        Option::<&gtk::Expression>::None,
    );
    archive_order.set_selected(0);
    let archive_media = DropDown::new(
        Some(StringList::new(&["All", "Images", "Videos"])),
        Option::<&gtk::Expression>::None,
    );
    archive_media.set_selected(0);
    let ar = filter_page_box();
    ar.append(&labeled_row("Sort", &archive_order));
    ar.append(&labeled_row("Media", &archive_media));
    stack.add_named(&ar, Some("archive"));

    let github_media = DropDown::new(
        Some(StringList::new(&["All", "Images", "Videos"])),
        Option::<&gtk::Expression>::None,
    );
    github_media.set_selected(0);
    let github_repos_all = CheckButton::with_label("All repos");
    github_repos_all.set_active(true);
    let github_repos_list = GtkBox::new(Orientation::Vertical, 2);
    let github_repos_scroll = ScrolledWindow::builder()
        .child(&github_repos_list)
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .max_content_height(220)
        .propagate_natural_height(true)
        .build();
    let github_repos_box = GtkBox::new(Orientation::Vertical, 6);
    github_repos_box.append(&filter_heading("Repos"));
    github_repos_box.append(&github_repos_all);
    github_repos_box.append(&github_repos_scroll);
    github_repos_box.set_visible(false);
    let gh = filter_page_box();
    gh.append(&labeled_row("Media", &github_media));
    gh.append(&github_repos_box);
    stack.add_named(&gh, Some("github"));

    let github_repo_checks = Rc::new(RefCell::new(Vec::new()));
    let github_repos_suppress = Rc::new(Cell::new(false));

    let none_lbl = Label::new(Some("No extra filters for this source"));
    none_lbl.add_css_class("dim-label");
    none_lbl.set_wrap(true);
    none_lbl.set_max_width_chars(28);
    none_lbl.set_margin_start(12);
    none_lbl.set_margin_end(12);
    none_lbl.set_margin_top(10);
    none_lbl.set_margin_bottom(10);
    stack.add_named(&none_lbl, Some("none"));

    let popover = Popover::new();
    popover.set_child(Some(&stack));
    // GTK #4529/#5414: dismiss outside clicks for nested popovers.
    let outside = GestureClick::new();
    outside.set_button(1);
    {
        let popover = popover.clone();
        outside.connect_pressed(move |_, _, x, y| {
            if !popover.contains(x, y) {
                popover.popdown();
            }
        });
    }
    popover.add_controller(outside);
    let button = MenuButton::new();
    button.set_label("Filters");
    button.set_popover(Some(&popover));

    DiscoverFilters {
        button,
        stack,
        purity_box,
        sfw,
        sketchy,
        nsfw,
        cat_general,
        cat_anime,
        cat_people,
        sort,
        top_range_row,
        top_range,
        atleast,
        ratios,
        hide_ai,
        load_full_preview,
        safesearch,
        pixabay_order,
        pixabay_category,
        pixabay_video_type,
        pixabay_video_type_row,
        pixabay_media,
        coverr_sort,
        archive_order,
        archive_media,
        github_media,
        github_repos_box,
        github_repos_list,
        github_repos_all,
        github_repo_checks,
        github_repos_suppress,
    }
}
