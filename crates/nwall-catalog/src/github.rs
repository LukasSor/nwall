use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use super::*;
use nwall_ipc::{is_image, is_video, CatalogSource};

/// True when `file_path` is a direct child of folder `path` (Contents-API semantics: no recursion).
pub(crate) fn github_path_is_direct_child(file_path: &str, folder: &str) -> bool {
    let file_path = file_path.trim_matches('/');
    let folder = folder.trim().trim_matches('/');
    if folder.is_empty() {
        return !file_path.is_empty() && !file_path.contains('/');
    }
    match file_path.strip_prefix(folder) {
        Some(rest) if rest.starts_with('/') => {
            let name = &rest[1..];
            !name.is_empty() && !name.contains('/')
        }
        _ => false,
    }
}

pub(crate) fn github_api_get(url: &str, token: &str) -> Result<ureq::Response> {
    github_api_get_class(url, token, RequestClass::Detail)
}

pub(crate) fn github_api_get_class(
    url: &str,
    token: &str,
    class: RequestClass,
) -> Result<ureq::Response> {
    let has_key = !token.trim().is_empty();
    note_source_auth("github", has_key);
    let mut headers = vec![
        ("Accept", "application/vnd.github+json"),
        ("X-GitHub-Api-Version", "2022-11-28"),
    ];
    let auth = if has_key {
        Some(format!("Bearer {}", token.trim()))
    } else {
        None
    };
    if let Some(auth) = auth.as_deref() {
        headers.push(("Authorization", auth));
    }
    limited_get_headers(url, "github", class, has_key, &headers)
}

pub(crate) fn github_err_is_rate_limit(err: &anyhow::Error) -> bool {
    http_err_is_rate_limit(err)
}

pub(crate) fn fetch_github_source(src: &CatalogSource) -> Result<Vec<RemoteItem>> {
    let targets = src.github_targets();
    if targets.is_empty() {
        return Err(anyhow!("github source '{}' has no repo list", src.name));
    }
    let token = source_api_key(src);

    // One Trees call per repo; keep target order.
    let mut by_repo: HashMap<String, Vec<String>> = HashMap::new();
    let mut repo_order: Vec<String> = Vec::new();
    for (repo, path) in targets {
        if !by_repo.contains_key(&repo) {
            repo_order.push(repo.clone());
        }
        by_repo.entry(repo).or_default().push(path);
    }
    let repos: Vec<(String, Vec<String>)> = repo_order
        .into_iter()
        .filter_map(|r| by_repo.remove(&r).map(|paths| (r, paths)))
        .collect();

    let concurrency = source_list_concurrency("github", !token.trim().is_empty());
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut errors = Vec::new();
    let mut rate_limited = false;

    for chunk in repos.chunks(concurrency) {
        if rate_limited {
            break;
        }
        let token = token.as_str();
        let batch: Vec<(String, Result<Vec<RemoteItem>>)> = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(chunk.len());
            for (repo, paths) in chunk {
                let repo = repo.clone();
                let paths = paths.clone();
                handles.push(scope.spawn(move || {
                    let res = fetch_github_repo(&repo, &paths, token);
                    (repo, res)
                }));
            }
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        // Re-attach in chunk order (thread::scope join order matches spawn order).
        for (repo, res) in batch {
            match res {
                Ok(list) => {
                    for it in list {
                        if seen.insert(it.url.clone()) {
                            items.push(it);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("github fetch {repo}: {e:#}");
                    if github_err_is_rate_limit(&e) {
                        rate_limited = true;
                        errors.push(e.to_string());
                    } else {
                        errors.push(format!("{repo}: {e:#}"));
                    }
                }
            }
        }
    }

    if items.is_empty() {
        if errors.is_empty() {
            return Ok(items);
        }
        // Deduplicate noisy per-repo rate-limit copies.
        let mut uniq = Vec::new();
        for e in errors {
            if !uniq.iter().any(|u: &String| u == &e) {
                uniq.push(e);
            }
        }
        return Err(anyhow!("github source '{}': {}", src.name, uniq.join("; ")));
    }
    if rate_limited {
        log::warn!(
            "github source '{}': rate-limited mid-fetch; returning {} items",
            src.name,
            items.len()
        );
    }
    // Images before videos so early pages have loadable thumbs.
    items.sort_by_key(|it| it.video);
    Ok(items)
}

/// List media under configured folders for one repo (tree API, Contents fallback).
pub(crate) fn fetch_github_repo(
    repo: &str,
    paths: &[String],
    token: &str,
) -> Result<Vec<RemoteItem>> {
    let mut items = match fetch_github_via_tree(repo, paths, token) {
        Ok(items) => items,
        Err(e) if github_err_is_rate_limit(&e) => return Err(e),
        Err(tree_err) => {
            log::warn!("github tree {repo}: {tree_err:#}; falling back to Contents API");
            let mut items = Vec::new();
            let mut errors = Vec::new();
            for path in paths {
                match fetch_github_contents(repo, path, token) {
                    Ok(list) => items.extend(list),
                    Err(e) if github_err_is_rate_limit(&e) => return Err(e),
                    Err(e) => {
                        log::warn!("github contents {repo}/{path}: {e:#}");
                        errors.push(format!("{path}: {e:#}"));
                    }
                }
            }
            if items.is_empty() && !errors.is_empty() {
                return Err(anyhow!("{}", errors.join("; ")));
            }
            items
        }
    };
    // One `/repos/{owner}/{repo}` per unique repo (cached): stamp topics on every file.
    apply_github_repo_topics(&mut items, repo, token);
    Ok(items)
}

pub(crate) const GITHUB_LFS_POINTER_MAX_BYTES: u64 = 256;

pub(crate) fn github_size_looks_like_lfs_pointer(size: Option<u64>) -> bool {
    matches!(size, Some(n) if (1..=GITHUB_LFS_POINTER_MAX_BYTES).contains(&n))
}

/// Blob download URL (media.githubusercontent.com for LFS pointers).
pub(crate) fn github_blob_download_url(repo: &str, path: &str, size: Option<u64>) -> String {
    let enc = percent_encode_path(path.trim_start_matches('/'));
    if github_size_looks_like_lfs_pointer(size) {
        format!("https://media.githubusercontent.com/media/{repo}/HEAD/{enc}")
    } else {
        format!("https://raw.githubusercontent.com/{repo}/HEAD/{enc}")
    }
}

pub(crate) fn fetch_github_via_tree(
    repo: &str,
    paths: &[String],
    token: &str,
) -> Result<Vec<RemoteItem>> {
    if repo.is_empty() {
        return Err(anyhow!("github source missing owner/repo"));
    }
    let api = format!("https://api.github.com/repos/{repo}/git/trees/HEAD?recursive=1");
    let tree: GhTree = match github_api_get_class(&api, token, RequestClass::Listing) {
        Ok(resp) => resp.into_json().context("github tree json")?,
        Err(e) if http_err_is_timeout(&e) => {
            log::warn!("github tree {repo}: {e:#}; retrying once");
            std::thread::sleep(Duration::from_millis(500));
            github_api_get_class(&api, token, RequestClass::Listing)?
                .into_json()
                .context("github tree json")?
        }
        Err(e) => return Err(e),
    };
    if tree.truncated {
        return Err(anyhow!(
            "github tree for {repo} truncated; use Contents fallback"
        ));
    }

    let folder_set: Vec<String> = paths
        .iter()
        .map(|p| p.trim().trim_matches('/').to_string())
        .collect();

    let mut items = Vec::new();
    for e in tree.tree {
        if e.typ != "blob" {
            continue;
        }
        if !folder_set
            .iter()
            .any(|f| github_path_is_direct_child(&e.path, f))
        {
            continue;
        }
        let name = e
            .path
            .rsplit('/')
            .next()
            .unwrap_or(e.path.as_str())
            .to_string();
        let p = PathBuf::from(&name);
        let video = is_video(&p);
        let image = is_image(&p);
        if !video && !image {
            continue;
        }
        let lfs = github_size_looks_like_lfs_pointer(e.size);
        let url = github_blob_download_url(repo, &e.path, e.size);
        let thumb = if image { Some(url.clone()) } else { None };
        let file_type = p
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.to_ascii_lowercase());
        items.push(RemoteItem {
            name,
            video,
            url,
            thumb,
            credit: Some("GitHub".into()),
            id: Some(e.path.clone()),
            repo: Some(repo.to_string()),
            file_size: if lfs { None } else { e.size.filter(|n| *n > 0) },
            file_type,
            page_url: Some(format!(
                "https://github.com/{repo}/blob/HEAD/{}",
                percent_encode_path(e.path.trim_start_matches('/'))
            )),
            ..Default::default()
        });
    }
    Ok(items)
}

pub(crate) fn fetch_github_contents(
    repo: &str,
    path: &str,
    token: &str,
) -> Result<Vec<RemoteItem>> {
    if repo.is_empty() {
        return Err(anyhow!("github source missing owner/repo"));
    }
    let api = if path.is_empty() {
        format!("https://api.github.com/repos/{repo}/contents")
    } else {
        format!(
            "https://api.github.com/repos/{repo}/contents/{}",
            percent_encode_path(path)
        )
    };
    let entries: Vec<GhEntry> = github_api_get_class(&api, token, RequestClass::Listing)?
        .into_json()
        .context("github json")?;
    let mut items = Vec::new();
    for e in entries {
        if e.typ != "file" {
            continue;
        }
        let p = PathBuf::from(&e.name);
        let video = is_video(&p);
        let image = is_image(&p);
        if !video && !image {
            continue;
        }
        let repo_path = if path.trim().is_empty() {
            e.name.clone()
        } else {
            format!("{}/{}", path.trim_matches('/'), e.name)
        };
        // Prefer Contents `download_url` (already LFS-aware). Fall back to constructed URL.
        let lfs = github_size_looks_like_lfs_pointer(e.size);
        let url = e
            .download_url
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| github_blob_download_url(repo, &repo_path, e.size));
        let thumb = if image { Some(url.clone()) } else { None };
        let file_type = PathBuf::from(&e.name)
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.to_ascii_lowercase());

        items.push(RemoteItem {
            name: e.name,
            video,
            url,
            thumb,
            credit: Some("GitHub".into()),
            id: Some(repo_path.clone()),
            repo: Some(repo.to_string()),
            file_size: if lfs { None } else { e.size.filter(|n| *n > 0) },
            file_type,
            page_url: Some(format!(
                "https://github.com/{repo}/blob/HEAD/{}",
                percent_encode_path(repo_path.trim_start_matches('/'))
            )),
            ..Default::default()
        });
    }
    Ok(items)
}

#[derive(Deserialize)]
pub(crate) struct GhTree {
    #[serde(default)]
    tree: Vec<GhTreeEntry>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize)]
pub(crate) struct GhTreeEntry {
    #[serde(default)]
    path: String,
    #[serde(rename = "type", default)]
    typ: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Deserialize)]
pub(crate) struct GhEntry {
    #[serde(default)]
    name: String,
    #[serde(rename = "type", default)]
    typ: String,
    download_url: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}
