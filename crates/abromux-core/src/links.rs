//! Links a workspace tracks (Jira tickets, GitLab merge requests, other
//! pages): their kinds and keys, and the parsers behind them. The sidebar's
//! list and opening them are the app's (`src/links.rs`).

/// `PAY-1423`: a Jira project key (a capital letter, then capitals, digits
/// or `_`) and a number.
pub fn is_jira_key(s: &str) -> bool {
    let Some((project, n)) = s.split_once('-') else {
        return false;
    };
    let mut c = project.chars();
    c.next().is_some_and(|f| f.is_ascii_uppercase())
        && project.len() >= 2
        && c.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
}

/// A merge request's web URL, `https://host[:port]/group/project/-/merge_requests/12[/diffs…]`,
/// as (host URL, project path, iid). GitLab under a sub-path isn't told
/// apart from a group: the first path segment is taken as one.
pub fn parse_mr_url(url: &str) -> Option<(String, String, u64)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let (authority, path) = rest.split_once('/')?;
    let (project, after) = path.split_once("/-/merge_requests/")?;
    let iid = after.split(['/', '?', '#']).next()?.parse().ok()?;
    (!project.is_empty()).then(|| (format!("{scheme}://{authority}"), project.to_string(), iid))
}

/// A merge request on the configured GitLab: its project's full path and
/// its number there.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MrKey {
    pub project: String,
    pub iid: u64,
}

impl MrKey {
    pub fn new(project: impl Into<String>, iid: u64) -> Self {
        Self {
            project: project.into(),
            iid,
        }
    }

    /// `group/project!12`, GitLab's own way to write it.
    pub fn reference(&self) -> String {
        format!("{}!{}", self.project, self.iid)
    }

    /// `https://host/group/project/-/merge_requests/12[/…]`, or
    /// `group/project!12`.
    pub fn parse(s: &str) -> Option<MrKey> {
        let s = s.trim();
        if let Some((project, iid)) = s.rsplit_once('!')
            && !project.contains("://")
        {
            let project = project.trim_matches('/');
            return (!project.is_empty() && project.contains('/'))
                .then(|| Some(MrKey::new(project, iid.trim().parse().ok()?)))
                .flatten();
        }
        if !(s.starts_with("https://") || s.starts_with("http://")) {
            return None;
        }
        // The key leaves the host out: one GitLab is set up at a time.
        let (_, project, iid) = parse_mr_url(s)?;
        Some(MrKey::new(&project, iid))
    }

    /// The project's last path segment.
    pub fn project_name(&self) -> &str {
        self.project.rsplit('/').next().unwrap_or(&self.project)
    }
}

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Jira,
    Mr,
    Web,
}

impl Kind {
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Jira => "jira",
            Kind::Mr => "gitlab",
            Kind::Web => "globe",
        }
    }

    /// Its icon's color: the brand's (Jira blue, GitLab orange).
    pub fn color(self) -> Option<u32> {
        match self {
            Kind::Jira => Some(0x2684ff),
            Kind::Mr => Some(0xfc6d26),
            Kind::Web => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Jira => "jira",
            Kind::Mr => "mr",
            Kind::Web => "web",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Link {
    pub kind: Kind,
    pub key: String,
    pub url: String,
    /// The page's title, as last seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Link {
    /// What the sidebar shows after the key.
    pub fn label(&self) -> Option<&str> {
        self.title
            .as_deref()
            .filter(|t| !t.is_empty() && *t != self.key)
    }

    /// The key as the sidebar shows it: a merge request by its project's
    /// last part (`api!812`), a page by its host and path.
    pub fn short_key(&self) -> String {
        match self.kind {
            Kind::Jira => self.key.clone(),
            Kind::Mr => MrKey::parse(&self.key)
                .map(|k| format!("{}!{}", k.project_name(), k.iid))
                .unwrap_or_else(|| self.key.clone()),
            Kind::Web => {
                let s = self.key.split_once("://").map_or(&*self.key, |(_, r)| r);
                s.trim_end_matches('/').to_string()
            }
        }
    }
}

fn is_web(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

/// The ticket a Jira URL shows: `/browse/KEY`, or `selectedIssue=KEY` on a
/// board.
fn jira_key_in_url(url: &str) -> Option<String> {
    let end = |s: &str| s.find(['/', '?', '#', '&']).unwrap_or(s.len());
    if let Some((_, rest)) = url.split_once("/browse/") {
        let k = &rest[..end(rest)];
        if is_jira_key(k) {
            return Some(k.into());
        }
    }
    if let Some((_, rest)) = url.split_once("selectedIssue=") {
        let k = &rest[..end(rest)];
        if is_jira_key(k) {
            return Some(k.into());
        }
    }
    None
}

/// `https://acme.atlassian.net` from `acme.atlassian.net` or a URL on it.
pub fn origin(s: &str) -> Option<String> {
    let s = s.trim();
    let (scheme, rest) = match s.split_once("://") {
        Some((sc, r)) => (sc, r),
        None => ("https", s),
    };
    let host = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

/// The kind and key of the page at `url`, if it's a web page.
pub fn key_of_url(url: &str) -> Option<(Kind, String)> {
    let url = url.trim();
    if !is_web(url) {
        return None;
    }
    if let Some(k) = jira_key_in_url(url) {
        return Some((Kind::Jira, k));
    }
    if let Some(k) = MrKey::parse(url) {
        return Some((Kind::Mr, k.reference()));
    }
    let page = url.split('#').next().unwrap_or(url).trim_end_matches('/');
    Some((Kind::Web, page.to_string()))
}

/// What `abromux link add` takes: a URL, `PAY-1423` (with `jira` the Jira
/// it's on), or `group/project!12` (with `mr_url` making its link).
pub fn parse(
    text: &str,
    jira: Option<&str>,
    mr_url: &dyn Fn(&MrKey) -> Option<String>,
) -> Result<Link, String> {
    let text = text.trim();
    if is_web(text) {
        let (kind, key) = key_of_url(text).ok_or_else(|| format!("not a link: {text:?}"))?;
        let url = match kind {
            // The merge request itself, not one of its pages, on the
            // GitLab the URL is on (which may not be the one set up).
            Kind::Mr => parse_mr_url(text)
                .map(|(host, project, iid)| format!("{host}/{project}/-/merge_requests/{iid}"))
                .unwrap_or_else(|| text.to_string()),
            Kind::Jira => match origin(text) {
                Some(o) => format!("{o}/browse/{key}"),
                None => text.to_string(),
            },
            Kind::Web => text.to_string(),
        };
        return Ok(Link {
            kind,
            key,
            url,
            title: None,
        });
    }
    if is_jira_key(text) {
        let base = jira.ok_or_else(|| {
            format!(
                "which Jira is {text} on? Give its URL, or set \"jira\": {{\"host\": \
                 \"acme.atlassian.net\"}} in the config file"
            )
        })?;
        return Ok(Link {
            kind: Kind::Jira,
            key: text.into(),
            url: format!("{base}/browse/{text}"),
            title: None,
        });
    }
    if let Some(k) = MrKey::parse(text) {
        let url = mr_url(&k)
            .ok_or_else(|| format!("{text}: GitLab isn't set up; give the merge request's URL"))?;
        return Ok(Link {
            kind: Kind::Mr,
            key: k.reference(),
            url,
            title: None,
        });
    }
    Err(format!(
        "not a link: {text:?} (give a URL, a Jira key like PAY-1423, or group/project!12)"
    ))
}

/// A page's title without what its site adds: `[PAY-1423] Refund fails -
/// Jira` → `Refund fails`.
pub fn clean_title(kind: Kind, key: &str, title: &str) -> Option<String> {
    let mut t = title.trim();
    if kind == Kind::Jira {
        if let Some(rest) = t.strip_prefix(&format!("[{key}]")) {
            t = rest.trim_start();
        }
        if let Some((head, tail)) = t.rsplit_once(" - ")
            && tail.contains("Jira")
        {
            t = head.trim_end();
        }
    }
    if kind == Kind::Mr
        && let Some((head, tail)) = t.split_once(" (!")
        && tail.contains(')')
    {
        t = head.trim_end();
    }
    (!t.is_empty() && t != key && !is_web(t)).then(|| t.to_string())
}

/// A merge request's title as the sidebar shows it: without GitLab's draft
/// marker (`Draft:`, `[Draft]`, `WIP:`) and the ticket it starts with
/// (`PAY-1423: `, `[PAY-1423] `), which get their own places. Gives the
/// title, whether it's a draft, and the ticket.
pub fn mr_title(title: &str) -> (String, bool, Option<String>) {
    let mut t = title.trim();
    let mut draft = false;
    loop {
        let lower = t.to_ascii_lowercase();
        let marker = ["draft:", "[draft]", "(draft)", "draft ", "wip:", "[wip]"]
            .iter()
            .find(|m| lower.starts_with(**m));
        match marker {
            Some(m) => {
                draft = true;
                t = t[m.len()..].trim_start();
            }
            None => break,
        }
    }
    // The ticket: bare or in brackets, then `:`, `-`, `|` or a space.
    let (inner, bracketed) = match t.strip_prefix('[').or_else(|| t.strip_prefix('(')) {
        Some(rest) => (rest, true),
        None => (t, false),
    };
    let end = inner
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(inner.len());
    let key = &inner[..end];
    let mut ticket = None;
    if is_jira_key(key) {
        let mut rest = &inner[end..];
        let closed = !bracketed || rest.starts_with([']', ')']);
        if bracketed && closed {
            rest = &rest[1..];
        }
        let rest = rest.trim_start_matches([':', '-', '|', '–', '—', ' ']);
        // A title that is only the ticket stays as it is.
        if closed && !rest.is_empty() {
            ticket = Some(key.to_string());
            t = rest;
        }
    }
    (t.to_string(), draft, ticket)
}

/// Merge requests stacked on one another (one's target branch is
/// another's source, in the same project): `branches[i]` is link i's
/// project, source and target branch if it's a merge request. Returns the
/// links in order, each after the one it's stacked on, with its depth.
pub fn stack_order(branches: &[Option<(&str, &str, &str)>]) -> Vec<(usize, usize)> {
    let n = branches.len();
    let parent: Vec<Option<usize>> = (0..n)
        .map(|i| {
            let (project, _, target) = branches[i]?;
            (0..n).find(|&j| {
                j != i && matches!(branches[j], Some((p, source, _)) if p == project && source == target)
            })
        })
        .collect();
    fn visit(
        i: usize,
        depth: usize,
        parent: &[Option<usize>],
        seen: &mut [bool],
        out: &mut Vec<(usize, usize)>,
    ) {
        if seen[i] {
            return;
        }
        seen[i] = true;
        out.push((i, depth));
        for j in 0..parent.len() {
            if parent[j] == Some(i) {
                visit(j, depth + 1, parent, seen, out);
            }
        }
    }
    let mut seen = vec![false; n];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        if parent[i].is_none() {
            visit(i, 0, &parent, &mut seen, &mut out);
        }
    }
    // A loop (a → b → a) has no first one: start it where the list does.
    for i in 0..n {
        visit(i, 0, &parent, &mut seen, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Kind, MrKey, clean_title, is_jira_key, key_of_url, origin, parse, stack_order};

    #[test]
    fn stacks_merge_requests() {
        let mr = |s: &'static str, t: &'static str| Some(("a/b", s, t));
        // 0 → main, 1 → feat-2 (on 2), 2 → feat-1 (on 0), 3 a ticket.
        let order = stack_order(&[
            mr("feat-1", "main"),
            mr("feat-3", "feat-2"),
            mr("feat-2", "feat-1"),
            None,
        ]);
        assert_eq!(order, [(0, 0), (2, 1), (1, 2), (3, 0)]);
        // Another project's branch of the same name isn't the base.
        let order = stack_order(&[Some(("x/y", "feat-1", "main")), mr("feat-2", "feat-1")]);
        assert_eq!(order, [(0, 0), (1, 0)]);
        // A loop still lists each once.
        let order = stack_order(&[mr("a", "b"), mr("b", "a")]);
        assert_eq!(order.len(), 2);
    }

    fn mr_url(k: &MrKey) -> Option<String> {
        Some(format!(
            "https://gitlab.acme.com/{}/-/merge_requests/{}",
            k.project, k.iid
        ))
    }

    #[test]
    fn jira_keys() {
        assert!(is_jira_key("PAY-1423"));
        assert!(is_jira_key("AB2_C-1"));
        assert!(!is_jira_key("P-1"));
        assert!(!is_jira_key("pay-1"));
        assert!(!is_jira_key("PAY-"));
        assert!(!is_jira_key("PAY-12a"));
    }

    #[test]
    fn keys_of_urls() {
        let k = |u| key_of_url(u).unwrap();
        assert_eq!(
            k("https://acme.atlassian.net/browse/PAY-1423"),
            (Kind::Jira, "PAY-1423".into())
        );
        assert_eq!(
            k(
                "https://acme.atlassian.net/jira/software/projects/PAY/boards/3?selectedIssue=PAY-7&x=1"
            ),
            (Kind::Jira, "PAY-7".into())
        );
        assert_eq!(
            k("https://gitlab.acme.com/acme/api/-/merge_requests/812/diffs#note_1"),
            (Kind::Mr, "acme/api!812".into())
        );
        assert_eq!(
            k("https://docs.rs/serde/#derive"),
            (Kind::Web, "https://docs.rs/serde".into())
        );
        assert_eq!(key_of_url("about:blank"), None);
    }

    #[test]
    fn parses_what_the_cli_takes() {
        let jira = Some("https://acme.atlassian.net");
        let l = parse("PAY-1423", jira, &mr_url).unwrap();
        assert_eq!(l.url, "https://acme.atlassian.net/browse/PAY-1423");
        assert!(parse("PAY-1423", None, &mr_url).is_err());
        let l = parse(
            "https://acme.atlassian.net/browse/PAY-9?focusedCommentId=1",
            None,
            &mr_url,
        )
        .unwrap();
        assert_eq!(
            (l.kind, &*l.key, &*l.url),
            (
                Kind::Jira,
                "PAY-9",
                "https://acme.atlassian.net/browse/PAY-9"
            )
        );
        let l = parse("acme/api!812", None, &mr_url).unwrap();
        assert_eq!(
            (l.kind, &*l.url),
            (
                Kind::Mr,
                "https://gitlab.acme.com/acme/api/-/merge_requests/812"
            )
        );
        let l = parse(
            "https://gitlab.acme.com/acme/api/-/merge_requests/812/diffs",
            None,
            &mr_url,
        )
        .unwrap();
        assert_eq!(
            (&*l.key, &*l.url),
            (
                "acme/api!812",
                "https://gitlab.acme.com/acme/api/-/merge_requests/812"
            )
        );
        // A merge request on another GitLab keeps its own host.
        let l = parse(
            "https://git.other.org/acme/api/-/merge_requests/5#note_1",
            None,
            &mr_url,
        )
        .unwrap();
        assert_eq!(
            (l.kind, &*l.url),
            (
                Kind::Mr,
                "https://git.other.org/acme/api/-/merge_requests/5"
            )
        );
        assert!(parse("hello", jira, &mr_url).is_err());
    }

    #[test]
    fn titles() {
        assert_eq!(
            clean_title(Kind::Jira, "PAY-1", "[PAY-1] Refund fails - Jira").as_deref(),
            Some("Refund fails")
        );
        assert_eq!(clean_title(Kind::Jira, "PAY-1", "PAY-1"), None);
        assert_eq!(
            clean_title(
                Kind::Mr,
                "a/b!3",
                "Fix rounding (!3) · Merge requests · a / b · GitLab"
            )
            .as_deref(),
            Some("Fix rounding")
        );
        assert_eq!(clean_title(Kind::Web, "x", "https://x.com"), None);
    }

    #[test]
    fn merge_request_titles() {
        let t = |s: &str| super::mr_title(s);
        assert_eq!(
            t("Draft: PAY-1423 Refund fails"),
            ("Refund fails".into(), true, Some("PAY-1423".into()))
        );
        assert_eq!(
            t("[PAY-1423] Refund fails"),
            ("Refund fails".into(), false, Some("PAY-1423".into()))
        );
        assert_eq!(
            t("PAY-1423: Refund fails"),
            ("Refund fails".into(), false, Some("PAY-1423".into()))
        );
        assert_eq!(
            t("Draft: Resolve \"PAY-1 x\""),
            ("Resolve \"PAY-1 x\"".into(), true, None)
        );
        assert_eq!(
            t("[Draft] WIP: Fix rounding"),
            ("Fix rounding".into(), true, None)
        );
        assert_eq!(
            t("PAY-1423 - Refund – fails"),
            ("Refund – fails".into(), false, Some("PAY-1423".into()))
        );
        // Not tickets, and a title that's only one.
        assert_eq!(
            t("UTF-8 everywhere"),
            ("everywhere".into(), false, Some("UTF-8".into()))
        );
        assert_eq!(t("PAY-1423"), ("PAY-1423".into(), false, None));
        assert_eq!(
            t("Drafting the plan"),
            ("Drafting the plan".into(), false, None)
        );
        assert_eq!(
            t("[PAY-1423 Refund"),
            ("[PAY-1423 Refund".into(), false, None)
        );
    }

    #[test]
    fn origins() {
        assert_eq!(
            origin("acme.atlassian.net").as_deref(),
            Some("https://acme.atlassian.net")
        );
        assert_eq!(
            origin("https://acme.atlassian.net/browse/X-1").as_deref(),
            Some("https://acme.atlassian.net")
        );
    }
}
