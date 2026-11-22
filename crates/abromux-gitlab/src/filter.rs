//! The merge request list's filter, as GitLab's filtered search reads it:
//! words to find in the title, reference, author or branches, and tokens
//! for the rest. `author:@alice`, `assignee:@bob`, `reviewer:@bob`,
//! `label:~backend` (`~"two words"`), `milestone:%v1.0`, `draft:yes`,
//! `target:main` and `source:fix-` (a prefix), each negated with a leading
//! `-`, and `none` or `any` for "has none" and "has some". Applied to the
//! list already loaded, so it narrows as you type.

use crate::types::MergeRequest;

/// The tokens the list's hint row offers, with what they insert.
pub const TOKENS: [(&str, &str); 8] = [
    ("Author", "author:@"),
    ("Assignee", "assignee:@"),
    ("Reviewer", "reviewer:@"),
    ("Label", "label:~"),
    ("Milestone", "milestone:%"),
    ("Draft", "draft:yes"),
    ("Target branch", "target:"),
    ("Source branch", "source:"),
];

#[derive(Debug, PartialEq)]
enum Term {
    Word(String),
    Token {
        key: String,
        value: String,
        not: bool,
    },
}

/// `q` split into words and tokens; quotes keep spaces in a value.
fn terms(q: &str) -> Vec<Term> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut parts = vec![];
    for ch in q.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    for p in parts {
        let (not, body) = match p.strip_prefix('-') {
            Some(b) if b.contains(':') => (true, b),
            _ => (false, p.as_str()),
        };
        match body.split_once(':') {
            Some((k, v)) if KEYS.contains(&k.to_lowercase().as_str()) => out.push(Term::Token {
                key: k.to_lowercase(),
                value: v.trim_start_matches(['@', '~', '%']).to_lowercase(),
                not,
            }),
            _ => out.push(Term::Word(p.to_lowercase())),
        }
    }
    out
}

const KEYS: [&str; 10] = [
    "author",
    "assignee",
    "reviewer",
    "label",
    "milestone",
    "draft",
    "target",
    "source",
    "target-branch",
    "source-branch",
];

/// Whether `m` is one `q` asks for.
pub fn matches(m: &MergeRequest, q: &str) -> bool {
    terms(q).iter().all(|t| match t {
        Term::Word(w) => {
            let hay = format!(
                "{} {} {} {} {} {}",
                m.title,
                m.references.full,
                m.author.name,
                m.author.username,
                m.source_branch,
                m.target_branch
            )
            .to_lowercase();
            hay.contains(w.as_str())
        }
        Term::Token { key, value, not } => {
            // Still being typed (`label:`): don't filter yet.
            if value.is_empty() {
                return true;
            }
            let hit = token(m, key, value);
            hit != *not
        }
    })
}

fn token(m: &MergeRequest, key: &str, v: &str) -> bool {
    let people = |us: &[crate::types::User]| match v {
        "none" => us.is_empty(),
        "any" => !us.is_empty(),
        _ => us
            .iter()
            .any(|u| u.username.to_lowercase() == v || u.name.to_lowercase() == v),
    };
    match key {
        "author" => people(std::slice::from_ref(&m.author)),
        "assignee" => people(&m.assignees),
        "reviewer" => people(&m.reviewers),
        "label" => match v {
            "none" => m.labels.is_empty(),
            "any" => !m.labels.is_empty(),
            _ => m.labels.iter().any(|l| l.name.to_lowercase() == v),
        },
        "milestone" => match (v, &m.milestone) {
            ("none", ms) => ms.is_none(),
            ("any", ms) => ms.is_some(),
            (_, Some(ms)) => ms.title.to_lowercase() == v,
            (_, None) => false,
        },
        "draft" => m.draft == matches!(v, "yes" | "true" | "1"),
        "target" | "target-branch" => m.target_branch.to_lowercase().starts_with(v),
        "source" | "source-branch" => m.source_branch.to_lowercase().starts_with(v),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{Term, matches, terms};
    use crate::types::{Label, MergeRequest, Milestone, User};

    fn mr() -> MergeRequest {
        let u = |n: &str| User {
            username: n.into(),
            name: n.to_uppercase(),
            ..Default::default()
        };
        MergeRequest {
            title: "Add rate limiter".into(),
            author: u("alice"),
            reviewers: vec![u("bob")],
            labels: vec![Label {
                name: "needs review".into(),
                ..Default::default()
            }],
            milestone: Some(Milestone {
                title: "v1.0".into(),
                ..Default::default()
            }),
            source_branch: "rate-limiter".into(),
            target_branch: "main".into(),
            ..Default::default()
        }
    }

    #[test]
    fn reads_tokens_and_quotes() {
        assert_eq!(
            terms(r#"rate -label:~"needs review""#),
            vec![
                Term::Word("rate".into()),
                Term::Token {
                    key: "label".into(),
                    value: "needs review".into(),
                    not: true
                }
            ]
        );
    }

    #[test]
    fn filters_as_gitlab_does() {
        let m = mr();
        assert!(matches(&m, "rate author:@alice"));
        assert!(!matches(&m, "author:@bob"));
        assert!(matches(&m, "reviewer:@bob assignee:none"));
        assert!(matches(&m, r#"label:~"needs review""#));
        assert!(!matches(&m, r#"-label:~"needs review""#));
        assert!(matches(&m, "milestone:%v1.0 draft:no"));
        assert!(matches(&m, "target:main source:rate"));
        assert!(!matches(&m, "source:fix"));
        // Half typed: no filtering on it yet.
        assert!(matches(&m, "label:"));
    }
}
