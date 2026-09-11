use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use nwall_ipc::is_video;
use super::*;

#[derive(Deserialize)]
pub(crate) struct IndexFile {
    #[serde(default)]
    items: Vec<IndexItem>,
}

#[derive(Deserialize)]
pub(crate) struct IndexItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    url: String,
    thumb: Option<String>,
}

pub(crate) fn fetch_index(url: &str) -> Result<Vec<RemoteItem>> {
    let text = listing_agent()
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .into_string()
        .context("index body")?;
    if let Ok(file) = serde_json::from_str::<IndexFile>(&text) {
        return Ok(file.items.into_iter().filter_map(index_item).collect());
    }
    if let Ok(list) = serde_json::from_str::<Vec<IndexItem>>(&text) {
        return Ok(list.into_iter().filter_map(index_item).collect());
    }
    Err(anyhow!("not a nwall catalog JSON"))
}

pub(crate) fn index_item(it: IndexItem) -> Option<RemoteItem> {
    if it.url.is_empty() {
        return None;
    }
    let p = PathBuf::from(&it.url);
    let video = it.kind.eq_ignore_ascii_case("video") || is_video(&p);
    Some(RemoteItem {
        name: if it.name.is_empty() {
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("wallpaper")
                .into()
        } else {
            it.name
        },
        video,
        url: it.url,
        thumb: it.thumb,
        credit: None,
        ..Default::default()
    })
}

