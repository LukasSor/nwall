//! Internet Archive audio / background-music catalog helpers.

use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use super::*;

const MUSIC_ROWS: u32 = 12;
const MUSIC_MAX_FILE_BYTES: u64 = 40_000_000;
const MUSIC_HTTP_TIMEOUT_SECS: u64 = 45;
const MUSIC_SEARCH_RETRIES: u32 = 3;

const COLLECTION_BIAS: &str = "collection:(freemusicarchive OR netlabels OR opensource_audio)";

const STARTER_TRACKS: &[(&str, &str, &str)] = &[
    (
        "nineinchnails_ghosts_I_IV",
        "Ghosts I–IV",
        "Nine Inch Nails",
    ),
    (
        "EastForest-TheEducationOfTheIndividualSoul",
        "The Education Of The Individual Soul",
        "East Forest",
    ),
    ("NS050", "Another Day, Another Way", "No-Source Netlabel"),
    (
        "PoodleplayArkestra-TheoryOfColour",
        "Theory Of Colour",
        "Poodleplay Arkestra",
    ),
    ("pcr089EmilDavydov-Sketches", "Sketches", "Emil Davydov"),
    (
        "Vkrsnl037CandlegravityAMomentForMyself",
        "A Moment for Myself",
        "Candlegravity",
    ),
    ("stqk011", "A Struggle Between Right or Wrong", "Zero Call"),
    ("mia049", "Instrumentally Ill EP", "Aphilas"),
];

#[derive(Clone, Debug, Default)]
pub struct MusicTrack {
    pub id: String,
    pub title: String,
    pub creator: Option<String>,
    /// Seconds if available.
    pub duration: Option<f64>,
    pub page_url: String,
    /// May be filled on resolve.
    pub download_url: Option<String>,
    pub size: Option<u64>,
    pub license_url: Option<String>,
    pub thumb_url: Option<String>,
    pub avatar: Option<String>,
}

fn music_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(UA)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(MUSIC_HTTP_TIMEOUT_SECS))
        .timeout(Duration::from_secs(MUSIC_HTTP_TIMEOUT_SECS + 15))
        .build()
}

fn track_from_starter(id: &str, title: &str, creator: &str) -> MusicTrack {
    MusicTrack {
        id: id.to_string(),
        title: title.to_string(),
        creator: Some(creator.to_string()),
        duration: None,
        page_url: format!("https://archive.org/details/{id}"),
        download_url: None,
        size: None,
        license_url: None,
        thumb_url: None,
        avatar: None,
    }
}

/// Curated tracks for the Archive music homepage (instant, no network).
pub fn archive_music_starters() -> Vec<MusicTrack> {
    STARTER_TRACKS
        .iter()
        .map(|(id, title, creator)| track_from_starter(id, title, creator))
        .collect()
}

/// Search Archive.org for freely licensed / open audio suitable as background music.
pub fn fetch_archive_music(query: &str, page: u32) -> Result<Vec<MusicTrack>> {
    let page = page.max(1);
    let q = query.trim();
    let solr = if q.is_empty() {
        format!("mediatype:audio AND collection:freemusicarchive")
    } else {
        format!("({q}) AND mediatype:audio AND {COLLECTION_BIAS}")
    };
    let url = format!(
        "https://archive.org/advancedsearch.php?q={}&fl[]=identifier&fl[]=title&fl[]=creator&fl[]=licenseurl&fl[]=runtime&fl[]=item_size&fl[]=downloads&rows={}&page={}&sort[]=downloads+desc&output=json",
        urlencoding_lite(&solr),
        MUSIC_ROWS,
        page
    );

    #[derive(Deserialize)]
    struct IaSearch {
        response: IaResponse,
    }
    #[derive(Deserialize)]
    struct IaResponse {
        #[serde(default)]
        docs: Vec<IaDoc>,
    }
    #[derive(Deserialize)]
    struct IaDoc {
        #[serde(default)]
        identifier: String,
        title: Option<String>,
        #[serde(default)]
        creator: Option<Value>,
        #[serde(default)]
        licenseurl: Option<Value>,
        #[serde(default)]
        runtime: Option<Value>,
        #[serde(default)]
        item_size: Option<Value>,
        #[serde(default)]
        downloads: Option<Value>,
    }

    let mut last_err = None;
    let parsed: IaSearch = {
        let mut ok = None;
        for attempt in 1..=MUSIC_SEARCH_RETRIES {
            match music_agent().get(&url).call() {
                Ok(resp) => match resp.into_json::<IaSearch>() {
                    Ok(p) => {
                        ok = Some(p);
                        break;
                    }
                    Err(e) => {
                        last_err = Some(anyhow!("archive.org music json: {e}"));
                    }
                },
                Err(e) => {
                    last_err = Some(anyhow!("archive.org music search: {e}"));
                }
            }
            if attempt < MUSIC_SEARCH_RETRIES {
                std::thread::sleep(Duration::from_millis(400 * u64::from(attempt)));
            }
        }
        ok.ok_or_else(|| last_err.unwrap_or_else(|| anyhow!("archive.org music search failed")))?
    };

    let mut docs: Vec<IaDoc> = parsed
        .response
        .docs
        .into_iter()
        .filter(|d| !d.identifier.is_empty())
        .take(MUSIC_ROWS as usize)
        .collect();

    docs.sort_by(|a, b| {
        let la = json_stringish(a.licenseurl.as_ref()).is_some();
        let lb = json_stringish(b.licenseurl.as_ref()).is_some();
        lb.cmp(&la).then_with(|| {
            let da = json_stringish(a.downloads.as_ref())
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| a.downloads.as_ref().and_then(|v| v.as_u64()))
                .unwrap_or(0);
            let db = json_stringish(b.downloads.as_ref())
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| b.downloads.as_ref().and_then(|v| v.as_u64()))
                .unwrap_or(0);
            db.cmp(&da)
        })
    });

    let tracks = docs
        .into_iter()
        .map(|doc| {
            let id = doc.identifier;
            let title = doc
                .title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| id.clone());
            let duration =
                json_stringish(doc.runtime.as_ref()).and_then(|s| parse_archive_runtime(&s));
            let size = json_stringish(doc.item_size.as_ref())
                .and_then(|s| s.parse().ok())
                .or_else(|| doc.item_size.as_ref().and_then(|v| v.as_u64()));
            MusicTrack {
                page_url: format!("https://archive.org/details/{id}"),
                title,
                creator: json_stringish(doc.creator.as_ref()),
                duration,
                download_url: None,
                size,
                license_url: json_stringish(doc.licenseurl.as_ref()),
                thumb_url: None,
                avatar: None,
                id,
            }
        })
        .collect();

    Ok(tracks)
}

/// Resolve a concrete audio file URL under an Archive.org item.
pub fn resolve_archive_music_download(id: &str) -> Result<(String, Option<u64>, Option<f64>)> {
    let id = id.trim();
    if id.is_empty() {
        return Err(anyhow!("empty archive.org music id"));
    }
    let meta_url = format!("https://archive.org/metadata/{}", urlencoding_lite(id));
    let mut last_err = None;
    let raw: Value = {
        let mut ok = None;
        for attempt in 1..=MUSIC_SEARCH_RETRIES {
            match music_agent().get(&meta_url).call() {
                Ok(resp) => match resp.into_json::<Value>() {
                    Ok(v) => {
                        ok = Some(v);
                        break;
                    }
                    Err(e) => last_err = Some(anyhow!("archive.org metadata json {id}: {e}")),
                },
                Err(e) => last_err = Some(anyhow!("archive.org metadata {id}: {e}")),
            }
            if attempt < MUSIC_SEARCH_RETRIES {
                std::thread::sleep(Duration::from_millis(400 * u64::from(attempt)));
            }
        }
        ok.ok_or_else(|| last_err.unwrap_or_else(|| anyhow!("archive.org metadata {id} failed")))?
    };

    let md = raw.get("metadata").cloned().unwrap_or(Value::Null);
    let runtime = json_stringish(md.get("runtime")).and_then(|s| parse_archive_runtime(&s));

    let files = raw
        .get("files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let file = pick_archive_music_file(&files)
        .ok_or_else(|| anyhow!("archive.org: no suitable audio under size cap for '{id}'"))?;

    let name = json_stringish(file.get("name"))
        .ok_or_else(|| anyhow!("archive.org: audio file missing name for '{id}'"))?;
    let size = json_stringish(file.get("size")).and_then(|s| s.parse().ok());
    let duration = json_stringish(file.get("length"))
        .and_then(|s| parse_archive_runtime(&s))
        .or(runtime);
    let url = format!(
        "https://archive.org/download/{id}/{}",
        percent_encode_path(&name)
    );
    Ok((url, size, duration))
}

pub fn fetch_archive_music_duration(id: &str) -> Option<f64> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let meta_url = format!("https://archive.org/metadata/{}", urlencoding_lite(id));
    let raw: Value = music_agent().get(&meta_url).call().ok()?.into_json().ok()?;
    let md = raw.get("metadata").cloned().unwrap_or(Value::Null);
    if let Some(d) = json_stringish(md.get("runtime")).and_then(|s| parse_archive_runtime(&s)) {
        if d > 0.0 {
            return Some(d);
        }
    }
    let files = raw.get("files").and_then(|v| v.as_array())?;
    if let Some(file) = pick_archive_music_file(files) {
        if let Some(d) = json_stringish(file.get("length")).and_then(|s| parse_archive_runtime(&s))
        {
            if d > 0.0 {
                return Some(d);
            }
        }
    }
    // Fallback: longest audio length on the item.
    let mut best = 0.0_f64;
    for f in files {
        let name = json_stringish(f.get("name"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        if audio_ext_rank(&name).is_none() || archive_skip_file_name(&name) {
            continue;
        }
        if let Some(d) = json_stringish(f.get("length")).and_then(|s| parse_archive_runtime(&s)) {
            if d > best {
                best = d;
            }
        }
    }
    (best > 0.0).then_some(best)
}

/// Download a track into `~/.cache/nwall/remote/`, resolving the file URL if needed.
pub fn download_archive_music(track: &MusicTrack) -> Result<PathBuf> {
    let (url, _, _) = match &track.download_url {
        Some(u) if !u.trim().is_empty() => (u.clone(), track.size, track.duration),
        _ => resolve_archive_music_download(&track.id)?,
    };
    let hint = if track.title.trim().is_empty() {
        track.id.as_str()
    } else {
        track.title.as_str()
    };
    let mut last_err = None;
    for attempt in 1..=MUSIC_SEARCH_RETRIES {
        match download_music_file(&url, hint) {
            Ok(p) => return Ok(p),
            Err(e) => {
                last_err = Some(e);
                if attempt < MUSIC_SEARCH_RETRIES {
                    std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("music download failed")))
}

/// Prefer a playable mid-size mp3 (skip tiny 64kb derivatives when better exists).
fn pick_archive_music_file(files: &[Value]) -> Option<Value> {
    let mut candidates: Vec<(u8, u8, u64, Value)> = Vec::new();
    for f in files {
        let name = json_stringish(f.get("name")).unwrap_or_default();
        let n = name.to_ascii_lowercase();
        if n.is_empty() || archive_skip_file_name(&n) {
            continue;
        }
        let Some(format_rank) = audio_ext_rank(&n) else {
            continue;
        };
        let sz = json_stringish(f.get("size"))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        if sz != 0 && sz >= MUSIC_MAX_FILE_BYTES {
            continue;
        }
        // Prefer full encodes over IA `_64kb` / `_vbr` derivatives.
        let quality_penalty: u8 = if n.contains("_64kb") || n.contains("_32kb") {
            2
        } else if n.contains("_vbr") || n.contains("_128kb") {
            1
        } else {
            0
        };
        // Among same format+quality, prefer larger (better bitrate) but keep preview snappy.
        let size_key = if sz == 0 { 0 } else { sz.min(12_000_000) };
        candidates.push((format_rank, quality_penalty, size_key, f.clone()));
    }
    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| b.2.cmp(&a.2)) // larger first
    });
    candidates.into_iter().next().map(|(_, _, _, f)| f)
}

fn audio_ext_rank(name_lower: &str) -> Option<u8> {
    if name_lower.ends_with(".mp3") {
        Some(0)
    } else if name_lower.ends_with(".ogg") || name_lower.ends_with(".oga") {
        Some(1)
    } else if name_lower.ends_with(".flac") {
        Some(2)
    } else if name_lower.ends_with(".wav") {
        Some(3)
    } else if name_lower.ends_with(".m4a") {
        Some(4)
    } else if name_lower.ends_with(".opus") {
        Some(5)
    } else if name_lower.ends_with(".aac") {
        Some(6)
    } else {
        None
    }
}

/// Music download: longer timeout, lenient content-type for Archive CDN redirects.
fn download_music_file(url: &str, hint: &str) -> Result<PathBuf> {
    let dest = cached_path(url, hint);
    if dest.exists() && dest.metadata().map(|m| m.len() > 1024).unwrap_or(false) {
        return Ok(dest);
    }
    let tmp = download_part_path(&dest);
    let _ = std::fs::remove_file(&tmp);
    let resp = music_agent()
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    if let Some(ct) = resp.header("Content-Type") {
        let ct = ct.to_ascii_lowercase();
        let ok = ct.contains("audio/")
            || ct.contains("octet-stream")
            || ct.contains("mpeg")
            || ct.contains("ogg")
            || ct.contains("flac")
            || ct.is_empty();
        if !ok && (ct.contains("text/html") || ct.contains("application/json")) {
            return Err(anyhow!("GET {url}: got {ct} instead of audio"));
        }
    }
    let expected = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0);
    let mut file = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    let written = std::io::copy(&mut resp.into_reader(), &mut file)
        .with_context(|| format!("write {}", tmp.display()))?;
    drop(file);
    if written < 1024 {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!("GET {url}: audio too small ({written} bytes)"));
    }
    if let Some(exp) = expected {
        if written + 64 < exp && written * 2 < exp {
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow!("GET {url}: truncated ({written} of {exp} bytes)"));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        std::fs::copy(&tmp, &dest).with_context(|| format!("copy to {}: {e}", dest.display()))?;
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ignored network smoke test for Archive.org music search/download.
    #[test]
    #[ignore = "network: hits archive.org"]
    fn fetch_archive_music_and_download_smoke() {
        let tracks = fetch_archive_music("ambient", 1)
            .expect("fetch_archive_music should succeed with network");
        assert!(
            !tracks.is_empty(),
            "expected at least one ambient track from Archive.org"
        );
        eprintln!(
            "fetch_archive_music: {} tracks; sample: {:?} ({})",
            tracks.len(),
            tracks[0].title,
            tracks[0].id
        );

        // IA CDN can 5xx transiently; try a few candidates.
        let mut last_err = None;
        let mut downloaded = None;
        for track in tracks.iter().take(5) {
            match download_archive_music(track) {
                Ok(path) => {
                    downloaded = Some((track.clone(), path));
                    break;
                }
                Err(e) => {
                    eprintln!("download skip {}: {e:#}", track.id);
                    last_err = Some(e);
                }
            }
        }
        let (track, path) = downloaded.unwrap_or_else(|| {
            panic!(
                "download_archive_music failed for first 5 tracks: {:#}",
                last_err.unwrap_or_else(|| anyhow!("no attempts"))
            )
        });
        let path_str = path.to_string_lossy();
        assert!(path.is_file(), "downloaded path should exist: {path_str}");
        assert!(
            path_str.contains("nwall") && path_str.contains("remote"),
            "expected cache path under nwall/remote, got {path_str}"
        );
        eprintln!(
            "download_archive_music: {:?} ({}) -> {path_str} ({} bytes)",
            track.title,
            track.id,
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        );
    }

    #[test]
    #[ignore = "network"]
    fn download_each_starter() {
        for t in archive_music_starters() {
            eprintln!("trying {} ({})", t.title, t.id);
            match download_archive_music(&t) {
                Ok(p) => eprintln!("  OK {}", p.display()),
                Err(e) => eprintln!("  FAIL {e:#}"),
            }
        }
    }
}
