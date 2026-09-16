use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use super::*;

const MAX_RETRY_SLEEP: Duration = Duration::from_secs(20);
const SKIP_PREFETCH_COOLDOWN: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RequestClass {
    Listing,
    Detail,
    Thumb,
    Media,
}

#[derive(Clone, Copy, Debug)]
pub struct SourceBudget {
    pub timeout: Duration,
    pub api_concurrency: usize,
    pub api_min_interval: Duration,
    pub detail_concurrency: usize,
    pub thumb_concurrency: usize,
    pub thumb_min_interval: Duration,
    pub media_concurrency: usize,
    pub retry_attempts: u32,
    pub prefetch_delay: Duration,
}

#[derive(Clone, Copy)]
struct ClassBudget {
    concurrency: usize,
    min_interval: Duration,
}

struct Gate {
    inflight: usize,
    listing_waiters: usize,
    last: Instant,
    cooldown_until: Instant,
}

struct Registry {
    gates: HashMap<String, Gate>,
    auth: HashMap<String, bool>,
}

fn registry() -> &'static (Mutex<Registry>, Condvar) {
    static REG: OnceLock<(Mutex<Registry>, Condvar)> = OnceLock::new();
    REG.get_or_init(|| {
        (
            Mutex::new(Registry {
                gates: HashMap::new(),
                auth: HashMap::new(),
            }),
            Condvar::new(),
        )
    })
}

fn far_past() -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(3600))
        .unwrap_or_else(Instant::now)
}

pub fn note_source_auth(kind: &str, has_key: bool) {
    let kind = normalize_kind(kind);
    if let Ok(mut g) = registry().0.lock() {
        g.auth.insert(kind.to_string(), has_key);
    }
}

pub fn known_auth(kind: &str) -> bool {
    let kind = normalize_kind(kind);
    registry()
        .0
        .lock()
        .ok()
        .and_then(|g| g.auth.get(kind).copied())
        .unwrap_or(false)
}

pub fn source_label(kind: &str) -> &'static str {
    match normalize_kind(kind) {
        "wallhaven" => "Wallhaven",
        "github" => "GitHub",
        "nasa" => "NASA APOD",
        "pixabay" => "Pixabay",
        "coverr" => "Coverr",
        "bing" => "Bing",
        "archive" => "Internet Archive",
        "index" => "Catalog",
        _ => "Catalog",
    }
}

pub fn rate_limit_message(kind: &str) -> String {
    format!("{} rate limit, try again in a moment", source_label(kind))
}

pub fn timeout_message(kind: &str) -> String {
    format!("{} timed out, try again", source_label(kind))
}

pub fn http_err_is_rate_limit(err: &anyhow::Error) -> bool {
    err_text_is_rate_limit(&format!("{err:#}"))
}

pub fn http_err_is_timeout(err: &anyhow::Error) -> bool {
    err_text_is_timeout(&format!("{err:#}"))
}

pub fn err_text_is_rate_limit(err: &str) -> bool {
    let s = err.to_ascii_lowercase();
    s.contains("rate limit") || s.contains("rate-limited") || s.contains("429")
}

pub fn err_text_is_timeout(err: &str) -> bool {
    let s = err.to_ascii_lowercase();
    s.contains("timed out") || s.contains("timeout")
}

pub fn budget_for(kind: &str, has_key: bool) -> SourceBudget {
    match normalize_kind(kind) {
        "wallhaven" => {
            // Listing p50 ~3.65s / max ~3.72s (keyed+anon). Headers: 45/min either way.
            if has_key {
                SourceBudget {
                    timeout: Duration::from_secs(15),
                    api_concurrency: 1,
                    api_min_interval: Duration::from_millis(1400),
                    detail_concurrency: 2,
                    thumb_concurrency: 4,
                    thumb_min_interval: Duration::from_millis(40),
                    media_concurrency: 2,
                    retry_attempts: 3,
                    prefetch_delay: Duration::from_millis(750),
                }
            } else {
                SourceBudget {
                    timeout: Duration::from_secs(15),
                    api_concurrency: 1,
                    api_min_interval: Duration::from_millis(2000),
                    detail_concurrency: 1,
                    thumb_concurrency: 3,
                    thumb_min_interval: Duration::from_millis(80),
                    media_concurrency: 1,
                    retry_attempts: 3,
                    prefetch_delay: Duration::from_millis(1200),
                }
            }
        }
        "github" => {
            // Tree listing ~0.5–0.6s. PAT headers: 5000/h core. Unauth: 60/h.
            if has_key {
                SourceBudget {
                    timeout: Duration::from_secs(15),
                    api_concurrency: 2,
                    api_min_interval: Duration::from_millis(750),
                    detail_concurrency: 2,
                    thumb_concurrency: 4,
                    thumb_min_interval: Duration::from_millis(40),
                    media_concurrency: 3,
                    retry_attempts: 3,
                    prefetch_delay: Duration::from_millis(750),
                }
            } else {
                SourceBudget {
                    timeout: Duration::from_secs(15),
                    api_concurrency: 1,
                    api_min_interval: Duration::from_secs(60),
                    detail_concurrency: 1,
                    thumb_concurrency: 2,
                    thumb_min_interval: Duration::from_millis(80),
                    media_concurrency: 2,
                    retry_attempts: 2,
                    prefetch_delay: Duration::from_millis(1500),
                }
            }
        }
        "nasa" => {
            // Range listing p50 ~3.0s / max ~3.5s. DEMO_KEY headers: 10/hour (not 30).
            if has_key {
                SourceBudget {
                    timeout: Duration::from_secs(15),
                    api_concurrency: 1,
                    api_min_interval: Duration::from_millis(3600),
                    detail_concurrency: 1,
                    thumb_concurrency: 3,
                    thumb_min_interval: Duration::from_millis(50),
                    media_concurrency: 2,
                    retry_attempts: 3,
                    prefetch_delay: Duration::from_millis(750),
                }
            } else {
                SourceBudget {
                    timeout: Duration::from_secs(15),
                    api_concurrency: 1,
                    api_min_interval: Duration::from_secs(360),
                    detail_concurrency: 1,
                    thumb_concurrency: 2,
                    thumb_min_interval: Duration::from_millis(80),
                    media_concurrency: 1,
                    retry_attempts: 2,
                    prefetch_delay: Duration::from_millis(1500),
                }
            }
        }
        "pixabay" => SourceBudget {
            // Photo listing p50 ~2.47s. Headers: 100 / 60s.
            timeout: Duration::from_secs(12),
            api_concurrency: 1,
            api_min_interval: Duration::from_millis(700),
            detail_concurrency: 1,
            thumb_concurrency: 4,
            thumb_min_interval: Duration::from_millis(50),
            media_concurrency: 2,
            retry_attempts: 3,
            prefetch_delay: Duration::from_millis(800),
        },
        // Coverr Demo keys: 50 req/hour. No key in this install (401 in ~3.6s).
        "coverr" => SourceBudget {
            timeout: Duration::from_secs(15),
            api_concurrency: 1,
            api_min_interval: Duration::from_secs(75),
            detail_concurrency: 1,
            thumb_concurrency: 2,
            thumb_min_interval: Duration::from_millis(80),
            media_concurrency: 2,
            retry_attempts: 3,
            prefetch_delay: Duration::from_millis(800),
        },
        "bing" => SourceBudget {
            // HPImageArchive p50 ~2.24s / max ~2.27s. No rate-limit headers.
            timeout: Duration::from_secs(10),
            api_concurrency: 1,
            api_min_interval: Duration::from_millis(500),
            detail_concurrency: 1,
            thumb_concurrency: 3,
            thumb_min_interval: Duration::from_millis(50),
            media_concurrency: 2,
            retry_attempts: 3,
            prefetch_delay: Duration::from_millis(750),
        },
        "archive" => SourceBudget {
            // advancedsearch p50 ~0.68s / max ~0.70s. No rate-limit headers.
            timeout: Duration::from_secs(10),
            api_concurrency: 1,
            api_min_interval: Duration::from_millis(500),
            detail_concurrency: 1,
            thumb_concurrency: 2,
            thumb_min_interval: Duration::from_millis(80),
            media_concurrency: 2,
            retry_attempts: 3,
            prefetch_delay: Duration::from_millis(1000),
        },
        _ => SourceBudget {
            timeout: Duration::from_secs(12),
            api_concurrency: 1,
            api_min_interval: Duration::from_millis(200),
            detail_concurrency: 1,
            thumb_concurrency: 4,
            thumb_min_interval: Duration::from_millis(40),
            media_concurrency: 3,
            retry_attempts: 2,
            prefetch_delay: Duration::from_millis(750),
        },
    }
}

pub fn source_list_concurrency(kind: &str, has_key: bool) -> usize {
    budget_for(kind, has_key).api_concurrency.max(1)
}

pub fn source_detail_concurrency(kind: &str, has_key: bool) -> usize {
    budget_for(kind, has_key).detail_concurrency.max(1)
}

pub fn prefetch_delay_for(kind: &str, has_key: bool) -> Duration {
    budget_for(kind, has_key).prefetch_delay
}

pub fn parse_retry_after(header: Option<&str>) -> Option<Duration> {
    let s = header?.trim();
    if s.is_empty() {
        return None;
    }
    s.parse::<u64>()
        .ok()
        .map(|secs| Duration::from_secs(secs.clamp(1, 30)))
}

pub fn cooldown_remaining(kind: &str) -> Duration {
    let id = gate_id(normalize_kind(kind), RequestClass::Listing);
    let Ok(g) = registry().0.lock() else {
        return Duration::ZERO;
    };
    let Some(gate) = g.gates.get(&id) else {
        return Duration::ZERO;
    };
    gate.cooldown_until
        .saturating_duration_since(Instant::now())
}

pub fn should_skip_prefetch(kind: &str) -> bool {
    cooldown_remaining(kind) > SKIP_PREFETCH_COOLDOWN
}

pub fn wait_source_ready(kind: &str, timeout: Duration) -> bool {
    if should_skip_prefetch(kind) {
        return false;
    }
    let id = gate_id(normalize_kind(kind), RequestClass::Listing);
    let deadline = Instant::now() + timeout;
    let (mu, cv) = registry();
    loop {
        let Ok(g) = mu.lock() else {
            return false;
        };
        let idle = g.gates.get(&id).map(gate_idle).unwrap_or(true);
        if idle {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let wait = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(50));
        let _ = cv.wait_timeout(g, wait);
    }
}

pub fn wait_wallhaven_listing_ready(timeout: Duration) {
    let _ = wait_source_ready("wallhaven", timeout);
}

pub fn kind_from_url(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    let host = host_of(&lower);
    if host.contains("wallhaven") {
        "wallhaven"
    } else if host.contains("pixabay") {
        "pixabay"
    } else if host.contains("coverr") {
        "coverr"
    } else if host.contains("nasa.gov") {
        "nasa"
    } else if host.contains("github") {
        "github"
    } else if host.contains("archive.org") || host.contains("archive-it.org") {
        "archive"
    } else if host.contains("bing.com") || host.contains("bing.net") {
        "bing"
    } else {
        "index"
    }
}

pub fn limited_get(
    url: &str,
    kind: &str,
    class: RequestClass,
    has_key: bool,
) -> Result<ureq::Response> {
    limited_get_headers(url, kind, class, has_key, &[])
}

pub fn limited_get_headers(
    url: &str,
    kind: &str,
    class: RequestClass,
    has_key: bool,
    headers: &[(&str, &str)],
) -> Result<ureq::Response> {
    let kind = normalize_kind(kind);
    note_source_auth(kind, has_key);
    let budget = budget_for(kind, has_key);
    let attempts = budget.retry_attempts.max(1);
    for attempt in 0..attempts {
        let outcome = {
            let _guard = acquire(kind, class, has_key);
            let mut req = agent_for(kind, class).get(url).timeout(budget.timeout);
            for (k, v) in headers {
                req = req.set(k, v);
            }
            req.call()
        };
        match outcome {
            Ok(resp) => return Ok(resp),
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = resp.header("retry-after").map(str::to_string);
                let reset = resp.header("x-ratelimit-reset").map(str::to_string);
                let remaining = resp.header("x-ratelimit-remaining").map(str::to_string);
                let body = resp.into_string().unwrap_or_default();
                if status_is_rate_limit(code, &body, remaining.as_deref()) {
                    let wait = retry_wait(attempt, retry_after.as_deref(), reset.as_deref());
                    set_cooldown(kind, class, wait.unwrap_or(Duration::from_secs(8)));
                    log::warn!(
                        "{} HTTP {code} attempt {}/{} wait={}ms",
                        kind,
                        attempt + 1,
                        attempts,
                        wait.unwrap_or(Duration::ZERO).as_millis()
                    );
                    if let Some(wait) = wait {
                        if attempt + 1 < attempts {
                            std::thread::sleep(wait);
                            continue;
                        }
                    }
                    return Err(rate_limit_err(kind, has_key));
                }
                if code == 503 {
                    let wait = retry_wait(attempt, retry_after.as_deref(), None)
                        .unwrap_or_else(|| Duration::from_millis(400 * u64::from(attempt + 1)));
                    set_cooldown(kind, class, wait.min(MAX_RETRY_SLEEP));
                    if attempt + 1 < attempts && wait <= MAX_RETRY_SLEEP {
                        std::thread::sleep(wait.min(MAX_RETRY_SLEEP));
                        continue;
                    }
                    return Err(anyhow!("{} temporarily unavailable", source_label(kind)));
                }
                log::warn!("{} HTTP {code}", kind);
                return Err(http_status_err(kind, code));
            }
            Err(e) => {
                let msg = e.to_string();
                if err_text_is_timeout(&msg) {
                    log::warn!("{} timeout", kind);
                    if attempt + 1 < attempts {
                        std::thread::sleep(Duration::from_millis(200 * u64::from(attempt + 1)));
                        continue;
                    }
                    return Err(anyhow!("{}", timeout_message(kind)));
                }
                log::warn!("{} request failed: {msg}", kind);
                return Err(anyhow!("{} request failed", source_label(kind)));
            }
        }
    }
    Err(rate_limit_err(kind, has_key))
}

fn rate_limit_err(kind: &str, has_key: bool) -> anyhow::Error {
    if kind == "github" && !has_key {
        anyhow!("GitHub rate limit — add a PAT in Settings, or try again later")
    } else if kind == "nasa" && !has_key {
        anyhow!("NASA APOD rate limit — add an API key in Settings, or try again later")
    } else {
        anyhow!("{}", rate_limit_message(kind))
    }
}

fn http_status_err(kind: &str, code: u16) -> anyhow::Error {
    match code {
        401 => anyhow!("{} unauthorized", source_label(kind)),
        403 => anyhow!("{} forbidden", source_label(kind)),
        404 => anyhow!("{} not found", source_label(kind)),
        _ => anyhow!("{} HTTP {code}", source_label(kind)),
    }
}

fn status_is_rate_limit(code: u16, body: &str, remaining: Option<&str>) -> bool {
    if code == 429 {
        return true;
    }
    let lower = body.to_ascii_lowercase();
    if code == 403 && (lower.contains("rate limit") || lower.contains("secondary rate limit")) {
        return true;
    }
    if matches!(code, 403 | 429) && remaining.is_some_and(|r| r.trim() == "0") {
        return true;
    }
    false
}

fn retry_wait(attempt: u32, retry_after: Option<&str>, reset: Option<&str>) -> Option<Duration> {
    if let Some(d) = parse_retry_after(retry_after) {
        return if d <= MAX_RETRY_SLEEP { Some(d) } else { None };
    }
    if let Some(reset) = reset.and_then(|s| s.trim().parse::<u64>().ok()) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if reset > now {
            let wait = Duration::from_secs((reset - now).clamp(1, 3600));
            return if wait <= MAX_RETRY_SLEEP {
                Some(wait)
            } else {
                None
            };
        }
    }
    Some(
        Duration::from_millis(400 * u64::from(attempt.saturating_add(1)))
            .clamp(Duration::from_millis(400), MAX_RETRY_SLEEP),
    )
}

fn agent_for(kind: &str, class: RequestClass) -> ureq::Agent {
    match (kind, class) {
        ("github", RequestClass::Listing | RequestClass::Detail) => github_agent(),
        ("archive", RequestClass::Listing | RequestClass::Detail) => archive_agent(),
        (_, RequestClass::Thumb | RequestClass::Media) => agent(),
        _ => listing_agent(),
    }
}

fn class_budget(kind: &str, class: RequestClass, has_key: bool) -> ClassBudget {
    let b = budget_for(kind, has_key);
    match class {
        RequestClass::Listing | RequestClass::Detail => ClassBudget {
            concurrency: b.api_concurrency.max(1),
            min_interval: b.api_min_interval,
        },
        RequestClass::Thumb => ClassBudget {
            concurrency: b.thumb_concurrency.max(1),
            min_interval: b.thumb_min_interval,
        },
        RequestClass::Media => ClassBudget {
            concurrency: b.media_concurrency.max(1),
            min_interval: Duration::from_millis(30),
        },
    }
}

fn gate_id(kind: &str, class: RequestClass) -> String {
    match class {
        RequestClass::Listing | RequestClass::Detail => kind.to_string(),
        RequestClass::Thumb => format!("{kind}-thumb"),
        RequestClass::Media => format!("{kind}-media"),
    }
}

fn gate_idle(gate: &Gate) -> bool {
    gate.inflight == 0 && gate.cooldown_until <= Instant::now()
}

struct GateGuard {
    id: String,
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        let (mu, cv) = registry();
        if let Ok(mut g) = mu.lock() {
            if let Some(gate) = g.gates.get_mut(&self.id) {
                gate.inflight = gate.inflight.saturating_sub(1);
            }
            cv.notify_all();
        }
    }
}

fn acquire(kind: &str, class: RequestClass, has_key: bool) -> GateGuard {
    let id = gate_id(kind, class);
    let ClassBudget {
        concurrency,
        min_interval,
    } = class_budget(kind, class, has_key);
    let listing = class == RequestClass::Listing;
    let detail = class == RequestClass::Detail;
    let (mu, cv) = registry();
    if listing {
        if let Ok(mut g) = mu.lock() {
            gate_mut(&mut g, &id).listing_waiters += 1;
            cv.notify_all();
        }
    }
    loop {
        let Ok(mut g) = mu.lock() else {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        let now = Instant::now();
        {
            let gate = gate_mut(&mut g, &id);
            if gate.cooldown_until > now {
                let wait = gate.cooldown_until.saturating_duration_since(now);
                drop(cv.wait_timeout(g, wait.min(Duration::from_millis(250))));
                continue;
            }
        }
        {
            let gate = gate_mut(&mut g, &id);
            if detail && gate.listing_waiters > 0 {
                drop(cv.wait_timeout(g, Duration::from_millis(50)));
                continue;
            }
            if gate.inflight >= concurrency {
                drop(cv.wait_timeout(g, Duration::from_millis(50)));
                continue;
            }
            let wait = min_interval.saturating_sub(now.saturating_duration_since(gate.last));
            if !wait.is_zero() {
                drop(g);
                std::thread::sleep(wait);
                continue;
            }
            gate.inflight += 1;
            gate.last = Instant::now();
            if listing {
                gate.listing_waiters = gate.listing_waiters.saturating_sub(1);
            }
        }
        return GateGuard { id };
    }
}

fn gate_mut<'a>(reg: &'a mut Registry, id: &str) -> &'a mut Gate {
    reg.gates.entry(id.to_string()).or_insert_with(|| Gate {
        inflight: 0,
        listing_waiters: 0,
        last: far_past(),
        cooldown_until: far_past(),
    })
}

fn set_cooldown(kind: &str, class: RequestClass, wait: Duration) {
    let id = gate_id(kind, class);
    let until = Instant::now() + wait;
    let (mu, cv) = registry();
    if let Ok(mut g) = mu.lock() {
        let gate = gate_mut(&mut g, &id);
        if until > gate.cooldown_until {
            gate.cooldown_until = until;
        }
        cv.notify_all();
    }
}

fn host_of(url: &str) -> &str {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

pub(crate) fn normalize_kind(kind: &str) -> &'static str {
    match kind.trim().to_ascii_lowercase().as_str() {
        "wallhaven" => "wallhaven",
        "github" => "github",
        "nasa" => "nasa",
        "pixabay" => "pixabay",
        "coverr" => "coverr",
        "bing" => "bing",
        "archive" => "archive",
        "index" => "index",
        other if other.contains("wallhaven") => "wallhaven",
        other if other.contains("github") => "github",
        other if other.contains("nasa") => "nasa",
        other if other.contains("pixabay") => "pixabay",
        other if other.contains("coverr") => "coverr",
        other if other.contains("bing") => "bing",
        other if other.contains("archive") => "archive",
        _ => "index",
    }
}

#[cfg(test)]
pub(crate) fn set_test_cooldown(kind: &str, wait: Duration) {
    set_cooldown(kind, RequestClass::Listing, wait);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallhaven_key_is_faster_than_anonymous() {
        let anon = budget_for("wallhaven", false);
        let key = budget_for("wallhaven", true);
        assert_eq!(anon.api_concurrency, 1);
        assert_eq!(key.api_concurrency, 1);
        assert!(key.api_min_interval < anon.api_min_interval);
        assert!(key.detail_concurrency > anon.detail_concurrency);
        assert!(key.prefetch_delay <= anon.prefetch_delay);
        assert_eq!(anon.timeout, Duration::from_secs(15));
        assert_eq!(key.timeout, Duration::from_secs(15));
    }

    #[test]
    fn github_token_raises_hourly_budget() {
        let anon = budget_for("github", false);
        let key = budget_for("github", true);
        assert_eq!(anon.api_concurrency, 1);
        assert!(anon.api_min_interval >= Duration::from_secs(60));
        assert!(key.api_concurrency >= 2);
        assert!(key.api_min_interval < anon.api_min_interval);
        assert!(key.api_min_interval >= Duration::from_millis(720));
        assert!(key.timeout >= anon.timeout);
        assert_eq!(anon.timeout, Duration::from_secs(15));
    }

    #[test]
    fn nasa_demo_key_is_conservative() {
        let demo = budget_for("nasa", false);
        let key = budget_for("nasa", true);
        assert_eq!(demo.api_concurrency, 1);
        assert!(key.api_min_interval < demo.api_min_interval);
        assert!(demo.api_min_interval >= Duration::from_secs(360));
        assert!(key.api_min_interval >= Duration::from_millis(3600));
    }

    #[test]
    fn pixabay_and_coverr_stay_serialized() {
        let pixabay = budget_for("pixabay", true);
        assert_eq!(pixabay.api_concurrency, 1);
        assert!(pixabay.api_min_interval >= Duration::from_millis(600));
        let coverr = budget_for("coverr", true);
        assert_eq!(coverr.api_concurrency, 1);
        assert!(coverr.api_min_interval >= Duration::from_secs(75));
    }

    #[test]
    fn archive_and_bing_are_polite() {
        let a = budget_for("archive", false);
        let b = budget_for("bing", false);
        assert_eq!(a.api_concurrency, 1);
        assert_eq!(b.api_concurrency, 1);
        assert_eq!(a.timeout, Duration::from_secs(10));
        assert_eq!(b.timeout, Duration::from_secs(10));
    }

    #[test]
    fn parse_retry_after_reads_seconds_and_clamps() {
        assert_eq!(parse_retry_after(None), None);
        assert_eq!(parse_retry_after(Some("")), None);
        assert_eq!(parse_retry_after(Some("  ")), None);
        assert_eq!(parse_retry_after(Some("5")), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after(Some("0")), Some(Duration::from_secs(1)));
        assert_eq!(parse_retry_after(Some("99")), Some(Duration::from_secs(30)));
        assert_eq!(
            parse_retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
    }

    #[test]
    fn skip_prefetch_only_on_long_cooldown() {
        set_test_cooldown("index", Duration::from_secs(60));
        assert!(should_skip_prefetch("index"));
        set_test_cooldown("index", Duration::from_millis(1));
        assert!(!should_skip_prefetch("bing"));
    }

    #[test]
    fn kind_from_url_maps_hosts() {
        assert_eq!(
            kind_from_url("https://wallhaven.cc/api/v1/search"),
            "wallhaven"
        );
        assert_eq!(
            kind_from_url("https://th.wallhaven.cc/small/ab/ab12.jpg"),
            "wallhaven"
        );
        assert_eq!(kind_from_url("https://api.github.com/repos/a/b"), "github");
        assert_eq!(
            kind_from_url("https://raw.githubusercontent.com/a/b/HEAD/x.jpg"),
            "github"
        );
        assert_eq!(kind_from_url("https://api.nasa.gov/planetary/apod"), "nasa");
        assert_eq!(kind_from_url("https://pixabay.com/api/?key=1"), "pixabay");
        assert_eq!(kind_from_url("https://api.coverr.co/videos"), "coverr");
        assert_eq!(
            kind_from_url("https://archive.org/advancedsearch.php?q=x"),
            "archive"
        );
        assert_eq!(
            kind_from_url("https://www.bing.com/HPImageArchive.aspx"),
            "bing"
        );
        assert_eq!(kind_from_url("https://example.com/catalog.json"), "index");
    }

    #[test]
    fn messages_are_short_and_url_free() {
        for kind in [
            "wallhaven",
            "github",
            "nasa",
            "pixabay",
            "coverr",
            "bing",
            "archive",
            "index",
        ] {
            let rl = rate_limit_message(kind);
            let to = timeout_message(kind);
            assert!(!rl.contains("http"), "{rl}");
            assert!(!to.contains("http"), "{to}");
            assert!(!rl.contains("://"), "{rl}");
            assert!(rl.contains("rate limit"), "{rl}");
            assert!(to.contains("timed out"), "{to}");
        }
        assert!(http_err_is_rate_limit(&anyhow!("status code 429")));
        assert!(http_err_is_timeout(&anyhow!("io: timed out")));
        assert!(!http_err_is_rate_limit(&anyhow!("Wallhaven HTTP 500")));
    }

    #[test]
    fn retry_wait_skips_hour_long_github_reset() {
        let far = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        assert_eq!(retry_wait(0, None, Some(&far.to_string())), None);
        assert_eq!(retry_wait(0, Some("5"), None), Some(Duration::from_secs(5)));
        assert_eq!(retry_wait(0, Some("60"), None), None);
    }
}
