use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use nwall_catalog::{self as catalog, RemoteItem};
use nwall_ipc::{is_image, is_video, Config};

pub enum RotateResult {
    Path(PathBuf),
    Failed(String),
}

pub fn spawn_pick(
    config: Config,
    current: Option<PathBuf>,
    busy: Arc<AtomicBool>,
    tx: calloop::channel::Sender<RotateResult>,
) {
    if busy.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Err(e) = std::thread::Builder::new()
        .name("nwall-rotate".into())
        .spawn({
            let busy = Arc::clone(&busy);
            move || {
                let result = match pick_next(&config, current.as_deref()) {
                    Ok(p) => RotateResult::Path(p),
                    Err(e) => {
                        log::warn!("slideshow pick failed: {e:#}");
                        RotateResult::Failed(format!("{e:#}"))
                    }
                };
                if let Err(e) = tx.send(result) {
                    log::warn!("slideshow: result channel closed ({e})");
                    busy.store(false, Ordering::SeqCst);
                }
            }
        })
    {
        log::warn!("slideshow thread: {e}");
        busy.store(false, Ordering::SeqCst);
    }
}

pub fn pick_next(config: &Config, current: Option<&Path>) -> Result<PathBuf> {
    let ss = &config.slideshow;
    if nwall_ipc::is_library_source(&ss.source) {
        return pick_library(&config.library, &ss.tags, current);
    }
    let mut sources = config.sources.clone();
    nwall_ipc::ensure_builtin_sources(&mut sources);
    let src = nwall_ipc::resolve_catalog_source(&sources, &ss.source)
        .cloned()
        .ok_or_else(|| {
            let names: Vec<&str> = sources.iter().map(|s| s.name.as_str()).collect();
            anyhow!(
                "slideshow source '{}' not in catalog (have: {})",
                ss.source,
                names.join(", ")
            )
        })?;
    let wallhaven = src.kind.eq_ignore_ascii_case("wallhaven");
    let mut opts = catalog::SearchOpts {
        query: ss.tags.clone(),
        page: 1,
        sorting: if wallhaven {
            "random".into()
        } else {
            String::new()
        },
        seed: if wallhaven {
            random_seed()
        } else {
            String::new()
        },
        ..Default::default()
    };
    let mut fetched = catalog::fetch_source(&src, &opts).map_err(|e| {
        log::warn!("slideshow fetch {}: {e:#}", src.name);
        e
    })?;
    if !wallhaven {
        if let Some(last) = fetched.last_page {
            if last > 1 {
                opts.page = (rand_index(last as usize) as u32).saturating_add(1);
                match catalog::fetch_source(&src, &opts) {
                    Ok(f) if !f.items.is_empty() => fetched = f,
                    Ok(_) => log::warn!(
                        "slideshow: random page {} of {} empty, using first page",
                        opts.page,
                        src.name
                    ),
                    Err(e) => log::warn!("slideshow: random page fetch {}: {e:#}", src.name),
                }
            }
        }
    }
    if fetched.items.is_empty() {
        log::warn!(
            "slideshow: no wallpapers from {} tags={:?}",
            src.name,
            ss.tags
        );
        return Err(anyhow!("no wallpapers from {}", src.name));
    }
    let item = pick_item(&fetched.items, current)?;
    catalog::download(&item.url, &item.file_hint()).map_err(|e| {
        log::warn!("slideshow download {}: {e:#}", item.url);
        e
    })
}

fn pick_item(items: &[RemoteItem], current: Option<&Path>) -> Result<RemoteItem> {
    let current_canon = current.and_then(|p| std::fs::canonicalize(p).ok());
    let not_current: Vec<&RemoteItem> = items
        .iter()
        .filter(|it| !item_is_current(it, current, current_canon.as_deref()))
        .collect();
    let pool = if not_current.is_empty() {
        items.iter().collect::<Vec<_>>()
    } else {
        not_current
    };
    let idx = rand_index(pool.len());
    pool.get(idx)
        .copied()
        .cloned()
        .ok_or_else(|| anyhow!("empty slideshow pool"))
}

fn item_is_current(item: &RemoteItem, current: Option<&Path>, current_canon: Option<&Path>) -> bool {
    let Some(cur) = current else {
        return false;
    };
    let hint = item.file_hint();
    let dest = catalog::cached_path(&item.url, &hint);
    if dest == cur {
        return true;
    }
    if let (Ok(a), Some(b)) = (std::fs::canonicalize(&dest), current_canon) {
        if a == b {
            return true;
        }
    }
    let stem = cur.file_stem().and_then(|n| n.to_str());
    stem == Some(hint.as_str()) || stem == Some(item.name.as_str())
}

fn pick_library(dir: &Path, tags: &str, current: Option<&Path>) -> Result<PathBuf> {
    let mut files = Vec::new();
    scan_library(dir, tags, &mut files, 0);
    if files.is_empty() {
        return Err(anyhow!(
            "no matching wallpapers in library {}",
            dir.display()
        ));
    }
    let current_canon = current.and_then(|p| std::fs::canonicalize(p).ok());
    let not_current: Vec<&PathBuf> = files
        .iter()
        .filter(|p| {
            if let Some(c) = current_canon.as_deref() {
                std::fs::canonicalize(p).ok().as_deref() != Some(c)
            } else {
                current.map(|c| c != p.as_path()).unwrap_or(true)
            }
        })
        .collect();
    let pool = if not_current.is_empty() {
        files.iter().collect::<Vec<_>>()
    } else {
        not_current
    };
    let idx = rand_index(pool.len());
    pool.get(idx)
        .copied()
        .cloned()
        .ok_or_else(|| anyhow!("empty library pool"))
}

fn scan_library(dir: &Path, tags: &str, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 4 || out.len() >= 2000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= 2000 {
            return;
        }
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            scan_library(&path, tags, out, depth + 1);
            continue;
        }
        if !(is_image(&path) || is_video(&path)) {
            continue;
        }
        if catalog::matches_query(name, tags) {
            out.push(path);
        }
    }
}

fn rand_u64() -> u64 {
    let mut buf = [0u8; 8];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
    {
        u64::from_le_bytes(buf)
    } else {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
    }
}

fn random_seed() -> String {
    format!("{:016x}", rand_u64())
}

fn rand_index(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (rand_u64() as usize) % len
}
