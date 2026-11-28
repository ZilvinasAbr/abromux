//! The Program Status Protocol (OSC 7501): a program tells the terminal
//! what it's doing (idle, working, done, blocked on the user, failed) and
//! why. Spec: <https://www.superlogical.com/rex/docs/build/program-status>
//! (revision 0.4). ConEmu's OSC 9;4 progress maps onto the root record
//! until a program sends a real report.
//!
//! A terminal keeps a set of records, one per id. Each report replaces its
//! record whole. `working` and `blocked` end with the process or at the
//! next shell prompt (OSC 133 A); `done` and `error` stay until replaced,
//! cleared, or seen.

use std::collections::HashMap;

/// The longest report, `OSC` through `ST`.
pub const MAX_SEQUENCE: usize = 4096;
const MAX_KEY: usize = 16;
const MAX_MSG_ENCODED: usize = 2732;
const MAX_MSG: usize = 2048;
const MAX_TITLE_ENCODED: usize = 256;
const MAX_TITLE: usize = 192;
const MAX_APP: usize = 32;
const MAX_ID: usize = 128;
const MAX_SEGMENT: usize = 32;
const MAX_DEPTH: usize = 8;
/// Records per terminal; the least recently updated goes past it.
const MAX_RECORDS: usize = 256;

/// The reply to `OSC 7501 ; ? ST`: supported, with the spec's default
/// states and kinds.
pub const DETECT_REPLY: &[u8] = b"\x1b]7501;?\x1b\\";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Working => "working",
            State::Done => "done",
            State::Blocked => "blocked",
            State::Error => "error",
        }
    }

    /// Which record a terminal shows when it has several: what needs the
    /// user first.
    fn rank(self) -> u8 {
        match self {
            State::Blocked => 4,
            State::Error => 3,
            State::Working => 2,
            State::Done => 1,
            State::Idle => 0,
        }
    }

    /// Ended by the process exiting or a new shell prompt.
    fn transient(self) -> bool {
        matches!(self, State::Working | State::Blocked | State::Idle)
    }
}

/// What a blocked program waits for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Permission,
    Question,
    Auth,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Permission => "permission",
            Kind::Question => "question",
            Kind::Auth => "auth",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Record {
    pub state: State,
    pub kind: Option<Kind>,
    /// 0–100; None is indeterminate.
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
}

impl Record {
    fn new(state: State) -> Self {
        Self {
            state,
            kind: None,
            progress: None,
            app: None,
            title: None,
            msg: None,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Report {
    /// Replace record `id` ("" is the root).
    Set { id: String, record: Record },
    /// Remove record `id` and everything beneath it; None removes them all.
    Clear { id: Option<String> },
    /// Feature detection: answer with [`DETECT_REPLY`].
    Query,
}

/// One report's body (what follows `7501;`). None when it is to be
/// ignored or discarded.
pub fn parse(body: &[u8]) -> Option<Report> {
    if body.len() > MAX_SEQUENCE {
        return None;
    }
    let body = std::str::from_utf8(body).ok()?;
    if body.trim() == "?" {
        return Some(Report::Query);
    }
    let mut pairs: HashMap<&str, &str> = HashMap::new();
    for pair in body.split(':') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.is_empty() || !k.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        if k.len() > MAX_KEY {
            return None;
        }
        if !v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
        {
            continue;
        }
        pairs.insert(k, v); // the last one wins
    }
    let id = match pairs.get("id") {
        None => String::new(),
        Some(id) => {
            if !valid_id(id) {
                return None;
            }
            id.to_string()
        }
    };
    let state = match *pairs.get("state")? {
        "idle" => State::Idle,
        "working" => State::Working,
        "done" => State::Done,
        "blocked" => State::Blocked,
        "error" => State::Error,
        "clear" => {
            return Some(Report::Clear {
                id: (!id.is_empty()).then_some(id),
            });
        }
        _ => return None,
    };
    let mut record = Record::new(state);
    if state == State::Blocked {
        record.kind = match pairs.get("kind").copied() {
            Some("permission") => Some(Kind::Permission),
            Some("question") => Some(Kind::Question),
            Some("auth") => Some(Kind::Auth),
            _ => None,
        };
    }
    if matches!(state, State::Working | State::Blocked) {
        record.progress = pairs
            .get("progress")
            .and_then(|p| p.parse::<u8>().ok())
            .filter(|p| *p <= 100 && !pairs["progress"].starts_with('+'));
    }
    record.app = pairs
        .get("app")
        .filter(|a| {
            (1..=MAX_APP).contains(&a.len())
                && a.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
        })
        .map(|a| a.to_string());
    record.title = text(pairs.get("title"), MAX_TITLE_ENCODED, MAX_TITLE)?;
    record.msg = text(pairs.get("msg"), MAX_MSG_ENCODED, MAX_MSG)?;
    Some(Report::Set { id, record })
}

/// A free-text value: base64 of one line of UTF-8. Ok(None) when absent;
/// None (discard the report) when too long, not base64, or not one line.
fn text(v: Option<&&str>, max_encoded: usize, max: usize) -> Option<Option<String>> {
    let Some(v) = v else { return Some(None) };
    if v.len() > max_encoded {
        return None;
    }
    let bytes = decode_base64(v)?;
    if bytes.len() > max {
        return None;
    }
    let s = String::from_utf8(bytes).ok()?;
    if s.chars().any(char::is_control) {
        return None;
    }
    Some(Some(s))
}

fn valid_id(id: &str) -> bool {
    let segments: Vec<&str> = id.split('/').collect();
    id.len() <= MAX_ID
        && segments.len() <= MAX_DEPTH
        && segments.iter().all(|s| {
            (1..=MAX_SEGMENT).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
        })
}

/// Standard base64, padding optional. None if it isn't.
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    if s.len() % 4 == 1 {
        return None;
    }
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let (mut out, mut acc, mut bits) = (Vec::with_capacity(s.len() * 3 / 4), 0u32, 0);
    for c in s.bytes() {
        acc = (acc << 6) | val(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Text from a record, safe to show outside the terminal: invisible
/// formatting and direction overrides removed.
pub fn display(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !matches!(c,
                '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
        })
        .collect()
}

/// OSC 9;4 (ConEmu progress): `st;pr` after `9;4;`. Some(None) clears the
/// root record.
pub fn conemu_progress(args: &[u8]) -> Option<Option<Record>> {
    let args = std::str::from_utf8(args).ok()?;
    let mut it = args.split(';');
    let st: u8 = it.next()?.trim().parse().ok()?;
    let pr = it
        .next()
        .and_then(|p| p.trim().parse::<u8>().ok())
        .map(|p| p.min(100));
    Some(match st {
        0 => None,
        1 | 4 => Some(Record {
            progress: pr.or(Some(0)),
            ..Record::new(State::Working)
        }),
        2 => Some(Record::new(State::Error)),
        3 => Some(Record::new(State::Working)),
        _ => return None,
    })
}

/// One terminal's records.
#[derive(Default)]
pub struct Records {
    map: HashMap<String, (Record, u64)>,
    /// Bumped on each update, for least-recently-updated eviction.
    tick: u64,
    /// A real OSC 7501 report came; OSC 9;4 is ignored until a reset.
    native: bool,
}

/// What a terminal shows for its records: the one that needs the user
/// most, with `app` inherited from its nearest ancestor.
#[derive(Clone, PartialEq, Debug)]
pub struct Shown {
    pub id: String,
    pub record: Record,
    /// Records besides this one.
    pub others: usize,
}

impl Records {
    /// Apply a report; true if anything changed.
    pub fn apply(&mut self, report: Report) -> bool {
        match report {
            Report::Query => false,
            Report::Clear { id } => {
                self.native = true;
                let before = self.map.len();
                match id {
                    None => self.map.clear(),
                    Some(id) => {
                        let prefix = format!("{id}/");
                        self.map.retain(|k, _| *k != id && !k.starts_with(&prefix));
                    }
                }
                before != self.map.len()
            }
            Report::Set { id, record } => {
                self.native = true;
                self.put(id, record)
            }
        }
    }

    /// OSC 9;4 for the root record, unless the program speaks OSC 7501.
    pub fn conemu(&mut self, record: Option<Record>) -> bool {
        if self.native {
            return false;
        }
        match record {
            Some(r) => self.put(String::new(), r),
            None => self.map.remove("").is_some(),
        }
    }

    fn put(&mut self, id: String, record: Record) -> bool {
        self.tick += 1;
        if let Some((old, at)) = self.map.get_mut(&id) {
            *at = self.tick;
            let changed = *old != record;
            *old = record;
            return changed;
        }
        if self.map.len() >= MAX_RECORDS
            && let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(k, _)| k.clone())
        {
            self.map.remove(&oldest);
        }
        self.map.insert(id, (record, self.tick));
        true
    }

    /// A new shell prompt (OSC 133 A) or the process exited: working,
    /// blocked and idle records end; done and error stay.
    pub fn prompt(&mut self) -> bool {
        let before = self.map.len();
        self.map.retain(|_, (r, _)| !r.state.transient());
        before != self.map.len()
    }

    /// The user has seen the terminal (typed in it): done and error records go.
    pub fn seen(&mut self) -> bool {
        let before = self.map.len();
        self.map
            .retain(|_, (r, _)| !matches!(r.state, State::Done | State::Error));
        before != self.map.len()
    }

    /// A full reset (RIS) removes every record and listens to OSC 9;4 again.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Every record, by id, with inherited `app`.
    pub fn list(&self) -> Vec<(String, Record)> {
        let mut out: Vec<(String, Record)> = self
            .map
            .keys()
            .map(|id| (id.clone(), self.resolved(id)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The record to show: blocked, then error, working, done; idle ones
    /// aren't shown. Among equals, the most recently updated.
    pub fn shown(&self) -> Option<Shown> {
        let (id, _) = self
            .map
            .iter()
            .filter(|(_, (r, _))| r.state != State::Idle)
            .max_by_key(|(_, (r, at))| (r.state.rank(), *at))?;
        Some(Shown {
            id: id.clone(),
            record: self.resolved(id),
            others: self.map.len() - 1,
        })
    }

    /// Record `id` with `app` from its nearest ancestor when it has none.
    fn resolved(&self, id: &str) -> Record {
        let mut r = self.map[id].0.clone();
        let mut at = id;
        while r.app.is_none() && !at.is_empty() {
            at = at.rsplit_once('/').map_or("", |(p, _)| p);
            r.app = self.map.get(at).and_then(|(p, _)| p.app.clone());
        }
        r
    }
}

/// A shown status record as JSON (`program.status`, `surface.list`), or
/// null for none.
pub fn to_json(shown: Option<&Shown>) -> serde_json::Value {
    let Some(s) = shown else {
        return serde_json::Value::Null;
    };
    let r = &s.record;
    serde_json::json!({
        "id": s.id,
        "state": r.state.as_str(),
        "app": r.app,
        "kind": r.kind.map(|k| k.as_str()),
        "progress": r.progress,
        "title": r.title,
        "msg": r.msg,
        "others": s.others,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json() {
        assert_eq!(to_json(None), serde_json::Value::Null);
        let mut record = Record::new(State::Working);
        record.progress = Some(40);
        record.title = Some("Building".into());
        let shown = Shown {
            id: "build".into(),
            record,
            others: 2,
        };
        assert_eq!(
            to_json(Some(&shown)),
            serde_json::json!({
                "id": "build", "state": "working", "app": null, "kind": null,
                "progress": 40, "title": "Building", "msg": null, "others": 2,
            })
        );
    }

    fn set(body: &str) -> Record {
        match parse(body.as_bytes()) {
            Some(Report::Set { record, .. }) => record,
            other => panic!("{body}: {other:?}"),
        }
    }

    #[test]
    fn parses_the_spec_examples() {
        let r = set(
            "state=blocked:kind=permission:app=terraform:msg=QXBwbHkgMyB0byBhZGQsIDEgdG8gY2hhbmdlLCAwIHRvIGRlc3Ryb3k/",
        );
        assert_eq!(r.state, State::Blocked);
        assert_eq!(r.kind, Some(Kind::Permission));
        assert_eq!(r.app.as_deref(), Some("terraform"));
        assert_eq!(
            r.msg.as_deref(),
            Some("Apply 3 to add, 1 to change, 0 to destroy?")
        );
        let r =
            set("state=working:id=us-east:title=VVMgRWFzdA==:progress=40:msg=UHVzaGluZyBpbWFnZQ==");
        assert_eq!(
            (r.title.as_deref(), r.progress),
            (Some("US East"), Some(40))
        );
        assert_eq!(
            parse(b"state=clear:id=us-east"),
            Some(Report::Clear {
                id: Some("us-east".into())
            })
        );
        assert_eq!(parse(b"state=clear"), Some(Report::Clear { id: None }));
        assert_eq!(parse(b"?"), Some(Report::Query));
    }

    #[test]
    fn ignores_and_discards_as_the_spec_says() {
        // No state, or an unknown one: ignored.
        assert_eq!(parse(b"app=x"), None);
        assert_eq!(parse(b"state=sleeping"), None);
        // Malformed pairs are skipped, the rest applies; the last key wins.
        let r = set("junk: =x:app=a b:state=idle:state=done: progress = 5 ");
        assert_eq!((r.state, r.app, r.progress), (State::Done, None, None));
        // kind only with blocked, progress only with working/blocked.
        assert_eq!(set("state=working:kind=auth").kind, None);
        assert_eq!(set("state=done:progress=5").progress, None);
        assert_eq!(set("state=working:progress=101").progress, None);
        assert_eq!(set("state=blocked:kind=nope").kind, None);
        // Bad base64, a newline in the text, or a bad id: discarded whole.
        assert_eq!(parse(b"state=done:msg=A"), None);
        // A byte outside the value set only skips its pair.
        assert_eq!(set("state=done:msg=@@@").msg, None);
        assert_eq!(parse(b"state=done:msg=YQpi"), None); // "a\nb"
        assert_eq!(parse(b"state=done:id=a//b"), None);
        assert_eq!(
            parse(format!("state=done:id={}", "a".repeat(33)).as_bytes()),
            None
        );
        // An app outside its character set is treated as absent.
        assert_eq!(set(&format!("state=done:app={}", "a".repeat(33))).app, None);
    }

    #[test]
    fn records_replace_clear_and_expire() {
        let mut r = Records::default();
        let rep = |b: &str| parse(b.as_bytes()).unwrap();
        assert!(r.apply(rep("state=working:app=deploy")));
        assert!(r.apply(rep("state=working:id=us-east:progress=40")));
        assert!(r.apply(rep("state=blocked:kind=permission:id=eu-west")));
        // Blocked wins, and inherits the root's app.
        let shown = r.shown().unwrap();
        assert_eq!(shown.id, "eu-west");
        assert_eq!(shown.record.app.as_deref(), Some("deploy"));
        assert_eq!(shown.others, 2);
        // A report replaces its record whole.
        assert!(r.apply(rep("state=working")));
        assert_eq!(r.list()[0].1.app, None);
        // Clearing a parent clears its children.
        r.apply(rep("state=done:id=eu-west/a"));
        assert!(r.apply(rep("state=clear:id=eu-west")));
        assert_eq!(r.list().len(), 2);
        // A prompt ends working ones; done survives until seen.
        r.apply(rep("state=done:id=us-east"));
        assert!(r.prompt());
        assert_eq!(r.shown().unwrap().record.state, State::Done);
        assert!(r.seen());
        assert!(r.is_empty());
        // Idle isn't shown.
        r.apply(rep("state=idle"));
        assert_eq!(r.shown(), None);
    }

    #[test]
    fn conemu_progress_until_a_real_report() {
        let mut r = Records::default();
        assert!(r.conemu(conemu_progress(b"1;50").unwrap()));
        assert_eq!(r.shown().unwrap().record.progress, Some(50));
        assert!(r.conemu(conemu_progress(b"0").unwrap()));
        assert!(r.is_empty());
        assert_eq!(conemu_progress(b"2").unwrap().unwrap().state, State::Error);
        r.apply(parse(b"state=working:msg=aGk=").unwrap());
        assert!(!r.conemu(conemu_progress(b"1;10").unwrap()));
        assert_eq!(r.shown().unwrap().record.msg.as_deref(), Some("hi"));
    }

    #[test]
    fn evicts_the_least_recently_updated() {
        let mut r = Records::default();
        for i in 0..MAX_RECORDS + 1 {
            r.apply(parse(format!("state=working:id=r{i}").as_bytes()).unwrap());
        }
        assert_eq!(r.list().len(), MAX_RECORDS);
        assert!(!r.list().iter().any(|(id, _)| id == "r0"));
    }

    #[test]
    fn display_strips_direction_overrides() {
        assert_eq!(display("a\u{202E}b\u{200B}c"), "abc");
    }
}
