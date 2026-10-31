//! Browser history, for the address bar's suggestions, as in cmux.
//!
//! A page that finishes loading counts as a visit; a URL entered in the
//! address bar (typed or picked from the suggestions) also counts as typed.
//! Suggestions rank matches by where the query matches (host, URL, title,
//! path) plus frecency: recent, often visited and often typed URLs rise.
//! The scores are cmux's, so the two rank alike.
//!
//! One history is shared by all browser panes and kept in
//! `browser-history.json` under Application Support.

use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_ENTRIES: usize = 5000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Unix seconds.
    pub last_visited: f64,
    pub visit_count: u32,
    #[serde(default)]
    pub typed_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_typed: Option<f64>,
}

/// The entries, most recently visited first.
#[derive(Default)]
pub struct History {
    entries: Vec<Entry>,
}

impl History {
    pub fn load(path: &Path) -> Self {
        let entries: Vec<Entry> = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut h = Self { entries };
        h.entries.retain(|e| key(&e.url).is_some());
        h.tidy();
        h
    }

    fn find(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|e| self::key(&e.url).as_deref() == Some(key))
    }

    /// Count a visit to `url`. False if it isn't kept (not http(s), say).
    pub fn record_visit(&mut self, url: &str, title: Option<&str>, now: f64) -> bool {
        let Some(k) = key(url) else { return false };
        let title = title.map(str::trim).filter(|t| !t.is_empty());
        match self.find(&k) {
            Some(i) => {
                let e = &mut self.entries[i];
                e.last_visited = now;
                e.visit_count += 1;
                // The URL as last loaded, after any redirect.
                e.url = url.into();
                if let Some(t) = title {
                    e.title = Some(t.into());
                }
            }
            None => self.entries.push(Entry {
                url: url.into(),
                title: title.map(Into::into),
                last_visited: now,
                visit_count: 1,
                typed_count: 0,
                last_typed: None,
            }),
        }
        self.tidy();
        true
    }

    /// Count `url` as entered in the address bar.
    pub fn record_typed(&mut self, url: &str, now: f64) -> bool {
        let Some(k) = key(url) else { return false };
        match self.find(&k) {
            Some(i) => {
                let e = &mut self.entries[i];
                e.typed_count += 1;
                e.last_typed = Some(now);
                e.last_visited = now;
            }
            None => self.entries.push(Entry {
                url: url.into(),
                title: None,
                last_visited: now,
                // The load that follows counts the visit.
                visit_count: 0,
                typed_count: 1,
                last_typed: Some(now),
            }),
        }
        self.tidy();
        true
    }

    /// Give `url`'s entry `title` (pages often set theirs after loading).
    pub fn set_title(&mut self, url: &str, title: &str) -> bool {
        let title = title.trim();
        let Some(i) = key(url).and_then(|k| self.find(&k)) else {
            return false;
        };
        if title.is_empty() || self.entries[i].title.as_deref() == Some(title) {
            return false;
        }
        self.entries[i].title = Some(title.into());
        true
    }

    pub fn remove(&mut self, url: &str) -> bool {
        let Some(i) = key(url).and_then(|k| self.find(&k)) else {
            return false;
        };
        self.entries.remove(i);
        true
    }

    /// Up to `limit` entries matching `query`, best first. An empty query
    /// gives the most typed, then most recent.
    pub fn suggestions(&self, query: &str, now: f64, limit: usize) -> Vec<Entry> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            let mut all: Vec<&Entry> = self.entries.iter().collect();
            all.sort_by(|a, b| {
                b.typed_count
                    .cmp(&a.typed_count)
                    .then(b.last_typed.unwrap_or(0.).total_cmp(&a.last_typed.unwrap_or(0.)))
                    .then(b.last_visited.total_cmp(&a.last_visited))
            });
            return all.into_iter().take(limit).cloned().collect();
        }
        let tokens = tokens(&q);
        let mut scored: Vec<(f64, &Entry)> = self
            .entries
            .iter()
            .filter_map(|e| score(e, &q, &tokens, now).map(|s| (s, e)))
            .collect();
        scored.sort_by(|(sa, a), (sb, b)| {
            sb.total_cmp(sa)
                .then(b.last_visited.total_cmp(&a.last_visited))
                .then(b.visit_count.cmp(&a.visit_count))
                .then(a.url.cmp(&b.url))
        });
        scored.into_iter().take(limit).map(|(_, e)| e.clone()).collect()
    }

    fn tidy(&mut self) {
        self.entries
            .sort_by(|a, b| b.last_visited.total_cmp(&a.last_visited));
        self.entries.truncate(MAX_ENTRIES);
    }
}

/// `url` without its scheme, its host (lowercase, no port) and its path
/// and query (no fragment).
fn parts(url_lower: &str) -> (&str, &str, &str) {
    let rest = url_lower
        .strip_prefix("https://")
        .or_else(|| url_lower.strip_prefix("http://"))
        .unwrap_or(url_lower);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host.rfind(':') {
        // Not inside an IPv6 literal.
        Some(i) if !host[i..].contains(']') => &host[..i],
        _ => host,
    };
    let path_query = &rest[end..];
    let path_query = &path_query[..path_query.find('#').unwrap_or(path_query.len())];
    (rest, host, path_query)
}

/// What makes two URLs the same page: http(s) only, ignoring `www.`, the
/// default port, trailing slashes and the fragment. None for URLs that
/// aren't kept, including hosts without a dot (a typo like `news.`), but
/// localhost and IP addresses are.
fn key(url: &str) -> Option<String> {
    let lower = url.trim().to_lowercase();
    let scheme = if lower.starts_with("https://") {
        "https"
    } else if lower.starts_with("http://") {
        "http"
    } else {
        return None;
    };
    let (rest, _, _) = parts(&lower);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.rfind(':') {
        Some(i) if !authority[i..].contains(']') => (&authority[..i], &authority[i + 1..]),
        _ => (authority, ""),
    };
    let host = host.trim_end_matches('.');
    let host = host.strip_prefix("www.").unwrap_or(host);
    let ok = host.contains('.') || host == "localhost" || host.starts_with('[');
    if host.is_empty() || !ok {
        return None;
    }
    let default = (scheme, port) == ("https", "443") || (scheme, port) == ("http", "80");
    let port = if port.is_empty() || default {
        String::new()
    } else {
        format!(":{port}")
    };
    let after = &rest[end..];
    let after = &after[..after.find('#').unwrap_or(after.len())];
    let (path, query) = after.split_once('?').unwrap_or((after, ""));
    let path = path.trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };
    let query = if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    };
    Some(format!("{scheme}://{host}{port}{path}{query}"))
}

/// The query's words, split at spaces and punctuation, without repeats.
fn tokens(q: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for t in q.split(|c: char| c.is_whitespace() || c.is_ascii_punctuation()) {
        if !t.is_empty() && !out.iter().any(|o| o == t) {
            out.push(t.into());
        }
    }
    out
}

/// cmux's score for `e` against lowercase `q`, or None if it doesn't match.
fn score(e: &Entry, q: &str, tokens: &[String], now: f64) -> Option<f64> {
    let url = e.url.to_lowercase();
    let (sans_scheme, host, path_query) = parts(&url);
    let title = e.title.as_deref().unwrap_or("").trim().to_lowercase();
    let url_value = if q.starts_with("http://") || q.starts_with("https://") {
        url.as_str()
    } else {
        sans_scheme
    };

    // One letter matches only at the start of the host, title or URL.
    if q.chars().count() == 1
        && !(host.starts_with(q) || title.starts_with(q) || url_value.starts_with(q))
    {
        return None;
    }
    let query_matches = url_value.contains(q)
        || host.contains(q)
        || path_query.contains(q)
        || title.contains(q);
    let token_matches = !tokens.is_empty()
        && tokens.iter().all(|t| {
            sans_scheme.contains(t.as_str())
                || host.contains(t.as_str())
                || path_query.contains(t.as_str())
                || title.contains(t.as_str())
        });
    if !query_matches && !token_matches {
        return None;
    }

    let mut s = 0.;
    let mut add = |cond: bool, points: f64| {
        if cond {
            s += points
        }
    };
    add(url_value == q, 1200.);
    add(host == q, 980.);
    add(host.starts_with(q), 680.);
    add(url_value.starts_with(q), 560.);
    add(title.starts_with(q), 420.);
    add(path_query.starts_with(q), 300.);
    add(host.contains(q), 210.);
    add(path_query.contains(q), 165.);
    add(title.contains(q), 145.);
    for t in tokens {
        let t = t.as_str();
        s += if host == t {
            260.
        } else if host.starts_with(t) {
            170.
        } else if host.contains(t) {
            110.
        } else {
            0.
        };
        s += if path_query.starts_with(t) {
            80.
        } else if path_query.contains(t) {
            52.
        } else {
            0.
        };
        s += if title.starts_with(t) {
            74.
        } else if title.contains(t) {
            48.
        } else {
            0.
        };
    }

    // Frecency.
    let age_hours = (now - e.last_visited).max(0.) / 3600.;
    s += (110. - age_hours / 3.).max(0.);
    s += ((e.visit_count.max(1) as f64).ln_1p() * 38.).min(120.);
    s += ((e.typed_count as f64).ln_1p() * 80.).min(190.);
    if let Some(typed) = e.last_typed {
        let typed_hours = (now - typed).max(0.) / 3600.;
        s += (85. - typed_hours / 4.).max(0.);
    }
    Some(s)
}

// The shared history, loaded on first use.

static STORE: Mutex<Option<History>> = Mutex::new(None);
/// Bumped by each change; a save goes ahead only if no later one came.
static CHANGES: AtomicU64 = AtomicU64::new(0);

fn path() -> PathBuf {
    crate::channel::support_dir().join("browser-history.json")
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0., |d| d.as_secs_f64())
}

fn with<R>(f: impl FnOnce(&mut History) -> R) -> R {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    f(store.get_or_insert_with(|| History::load(&path())))
}

/// Write the history soon, once changes stop for a moment.
fn save_soon() {
    let change = CHANGES.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        if CHANGES.load(Ordering::SeqCst) != change {
            return;
        }
        let Ok(json) = with(|h| serde_json::to_vec(&h.entries)) else {
            return;
        };
        let path = path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    });
}

pub fn visit(url: &str, title: Option<&str>) {
    if with(|h| h.record_visit(url, title, now())) {
        save_soon();
    }
}

pub fn typed(url: &str) {
    if with(|h| h.record_typed(url, now())) {
        save_soon();
    }
}

pub fn retitle(url: &str, title: &str) {
    if with(|h| h.set_title(url, title)) {
        save_soon();
    }
}

pub fn remove(url: &str) {
    if with(|h| h.remove(url)) {
        save_soon();
    }
}

pub fn suggest(query: &str, limit: usize) -> Vec<Entry> {
    with(|h| h.suggestions(query, now(), limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_800_000_000.;
    const DAY: f64 = 86_400.;

    fn urls(list: Vec<Entry>) -> Vec<String> {
        list.into_iter().map(|e| e.url).collect()
    }

    #[test]
    fn key_ignores_www_port_slash_and_fragment() {
        let k = key("https://github.com/").unwrap();
        assert_eq!(k, "https://github.com/");
        for same in [
            "https://www.github.com",
            "HTTPS://GitHub.com:443/",
            "https://github.com/#readme",
        ] {
            assert_eq!(key(same).as_deref(), Some(k.as_str()), "{same}");
        }
        assert_eq!(key("http://localhost:3000/a/").unwrap(), "http://localhost:3000/a");
        assert_eq!(key("https://a.com/x?Q=1").unwrap(), "https://a.com/x?q=1");
        assert_ne!(key("http://github.com"), key("https://github.com"));
    }

    #[test]
    fn key_rejects_other_schemes_and_dotless_hosts() {
        assert_eq!(key("about:blank"), None);
        assert_eq!(key("file:///tmp/a.html"), None);
        assert_eq!(key("https://news."), None);
        assert_eq!(key("https://intranet/"), None);
        assert!(key("http://127.0.0.1:8080/").is_some());
        assert!(key("http://[::1]:8080/").is_some());
    }

    #[test]
    fn visits_merge_by_key_and_keep_titles() {
        let mut h = History::default();
        assert!(h.record_visit("https://github.com/", Some("GitHub"), NOW));
        assert!(h.record_visit("https://www.github.com", Some("  "), NOW + 1.));
        assert!(!h.record_visit("about:blank", None, NOW));
        assert_eq!(h.entries.len(), 1);
        let e = &h.entries[0];
        assert_eq!(e.visit_count, 2);
        assert_eq!(e.title.as_deref(), Some("GitHub"));
        assert_eq!(e.url, "https://www.github.com");
    }

    #[test]
    fn host_prefix_beats_title_match() {
        let mut h = History::default();
        h.record_visit("https://docs.rs/gpui", Some("gpui - GitHub mirror"), NOW);
        h.record_visit("https://github.com/zed-industries/zed", Some("Zed"), NOW - DAY);
        assert_eq!(
            urls(h.suggestions("git", NOW, 10)),
            ["https://github.com/zed-industries/zed", "https://docs.rs/gpui"]
        );
    }

    #[test]
    fn single_letter_matches_only_prefixes() {
        let mut h = History::default();
        h.record_visit("https://github.com/", Some("GitHub"), NOW);
        h.record_visit("https://example.com/g", Some("Example"), NOW);
        assert_eq!(urls(h.suggestions("g", NOW, 10)), ["https://github.com/"]);
    }

    #[test]
    fn tokens_match_in_any_field() {
        let mut h = History::default();
        h.record_visit("https://github.com/zed-industries/zed/pulls", Some("Pull requests"), NOW);
        h.record_visit("https://github.com/rust-lang/rust", Some("Rust"), NOW);
        assert_eq!(
            urls(h.suggestions("zed pull", NOW, 10)),
            ["https://github.com/zed-industries/zed/pulls"]
        );
    }

    #[test]
    fn typed_and_frequent_urls_rank_higher() {
        let mut h = History::default();
        h.record_visit("https://news.ycombinator.com/", None, NOW);
        h.record_visit("https://newsletter.example.com/", None, NOW);
        h.record_typed("https://newsletter.example.com/", NOW);
        h.record_typed("https://newsletter.example.com/", NOW);
        assert_eq!(
            urls(h.suggestions("news", NOW, 10))[0],
            "https://newsletter.example.com/"
        );
    }

    #[test]
    fn empty_query_lists_typed_first() {
        let mut h = History::default();
        h.record_visit("https://a.com/", None, NOW);
        h.record_typed("https://b.com/", NOW - DAY);
        assert_eq!(urls(h.suggestions("", NOW, 10)), ["https://b.com/", "https://a.com/"]);
    }

    #[test]
    fn set_title_and_remove() {
        let mut h = History::default();
        h.record_visit("https://a.com/", None, NOW);
        assert!(h.set_title("https://www.a.com", "A"));
        assert_eq!(h.entries[0].title.as_deref(), Some("A"));
        assert!(!h.set_title("https://b.com/", "B"));
        assert!(h.remove("https://a.com"));
        assert!(h.entries.is_empty());
    }

    #[test]
    fn keeps_at_most_max_entries() {
        let mut h = History::default();
        for i in 0..MAX_ENTRIES + 5 {
            h.record_visit(&format!("https://e{i}.com/"), None, NOW + i as f64);
        }
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert_eq!(h.entries[0].url, format!("https://e{}.com/", MAX_ENTRIES + 4));
    }

    #[test]
    fn loads_what_it_saves() {
        let dir = std::env::temp_dir().join(format!("abromux-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("h.json");
        let mut h = History::default();
        h.record_visit("https://a.com/", Some("A"), NOW);
        h.record_typed("https://b.com/", NOW);
        std::fs::write(&path, serde_json::to_vec(&h.entries).unwrap()).unwrap();
        assert_eq!(History::load(&path).entries, h.entries);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
