
use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::StringList;
use nwall_catalog as catalog;
use nwall_ipc::{default_config_path, CatalogSource, Config};

pub(crate) type SourceWatchers = Rc<RefCell<Vec<Rc<dyn Fn()>>>>;

pub(crate) fn fire_source_watchers(watchers: &SourceWatchers) {
    let cbs: Vec<Rc<dyn Fn()>> = watchers.borrow().clone();
    for cb in cbs {
        cb();
    }
}

pub(crate) fn watch_sources(watchers: &SourceWatchers, f: impl Fn() + 'static) {
    watchers.borrow_mut().push(Rc::new(f));
}

pub(crate) fn visible_catalog_sources() -> Vec<CatalogSource> {
    let cfg = Config::load(&default_config_path()).unwrap_or_default();
    catalog::selectable_sources(&cfg.sources)
}

pub(crate) fn replace_string_list(list: &StringList, items: &[String]) {
    let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
    list.splice(0, list.n_items(), &refs);
}
