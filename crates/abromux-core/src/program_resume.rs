//! The Program Resume Protocol (OSC 7577): a program tells the terminal a
//! command that brings it back after the terminal restarts. Spec:
//! <https://www.superlogical.com/rex/docs/build/program-resume> (revision
//! 0.5).
//!
//! Each terminal gives its shell a random token in `OSC7577_TOKEN`. A report
//! carries the reporting process's pid, the time, the command (argv, each
//! element base64) and optionally a folder, then a `mac`: SHA-256 of
//! everything before `:mac=`, a `:`, and the token. Output that doesn't know
//! the token (`cat`, logs, ssh) can't make a record.
//!
//! A terminal keeps one record per owning process. A record goes when its
//! owner sends `clear=1`, when the owner exits (after a short grace, so
//! programs that die with the terminal stay recorded), or when the user
//! closes the terminal; it stays when the app quits or crashes. On restore
//! the shallowest record (the owner nearest the shell) runs again in the
//! new shell, through the shell integration (`src/shell/`), which reads its
//! argv from a one-time file ([`Restart::file_bytes`]) so no element is ever
//! read as shell code.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The environment variable the token goes in.
pub const ENV: &str = "OSC7577_TOKEN";
/// The longest body (between `7577;` and ST) taken; the spec's minimum for
/// the whole sequence is 2048 bytes.
pub const MAX_BODY: usize = 8192;
/// The most argv elements taken (the spec's minimum is 64).
const MAX_ARGV: usize = 256;
/// How far a report's time may be from now, either way.
const WINDOW: u64 = 60;
/// How long a record outlives its owner, so a program that exits as the
/// terminal ends (a reboot) is still resumed.
pub const GRACE: Duration = Duration::from_secs(2);
/// Keys this terminal implements, for `need`.
const KNOWN: [&str; 7] = ["pid", "time", "argv", "cwd", "clear", "need", "mac"];

/// A fresh token: 32 characters of `[A-Za-z0-9_-]` from the OS's CSPRNG.
pub fn new_token() -> String {
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).expect("the OS's random numbers");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// What a report says, once it's checked.
#[derive(Debug, PartialEq)]
pub enum Parsed {
    /// Not from anyone holding the token (bad or missing mac, stale time),
    /// or nothing to act on: changes nothing.
    Ignored,
    /// From the token's holder, but invalid: the owner's records go.
    Rejected {
        pid: u32,
        time: u64,
    },
    Report {
        pid: u32,
        time: u64,
        argv: Vec<String>,
        cwd: Option<String>,
    },
    Clear {
        pid: u32,
        time: u64,
    },
}

/// The MAC a body (everything before `:mac=`) has with `token`.
pub fn mac(body: &[u8], token: &str) -> String {
    let mut h = Sha256::new();
    h.update(body);
    h.update(b":");
    h.update(token.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Check and read one report body (after `OSC 7577 ;`, before ST), `now`
/// in Unix seconds.
pub fn parse(body: &[u8], token: &str, now: u64) -> Parsed {
    if token.is_empty() || body.len() > MAX_BODY {
        return Parsed::Ignored;
    }
    // `:` can't be in a value, so the last `:mac=` is the mac key; any `:`
    // after it means mac wasn't last.
    let Some(at) = body.windows(5).rposition(|w| w == b":mac=") else {
        return Parsed::Ignored;
    };
    let (signed, hex) = (&body[..at], &body[at + 5..]);
    if hex.len() != 64
        || !hex.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        || mac(signed, token).as_bytes() != hex
    {
        return Parsed::Ignored;
    }
    // Bytes that aren't UTF-8 become U+FFFD, which no key or value allows:
    // the report is signed, so it's rejected, not ignored.
    let signed = String::from_utf8_lossy(signed);
    // Read every pair, noting what's wrong, so pid and time are known
    // (to ignore or reject) even when something else isn't.
    let mut bad = false;
    let mut pairs: HashMap<&str, &str> = HashMap::new();
    for pair in signed.split(':') {
        let Some((k, v)) = pair.split_once('=') else {
            bad = true;
            continue;
        };
        if k.is_empty()
            || !k.bytes().all(|b| b.is_ascii_lowercase())
            || !v
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
            || pairs.insert(k, v).is_some()
        {
            bad = true;
        }
    }
    let number = |k| {
        pairs
            .get(k)
            .filter(|v: &&&str| v.bytes().all(|b| b.is_ascii_digit()) && !v.starts_with('0'))
            .and_then(|v| v.parse::<u64>().ok())
    };
    let (Some(time), Some(pid)) = (number("time"), number("pid")) else {
        return Parsed::Ignored;
    };
    let Ok(pid) = u32::try_from(pid) else {
        return Parsed::Ignored;
    };
    if time.abs_diff(now) > WINDOW {
        return Parsed::Ignored;
    }
    let rejected = Parsed::Rejected { pid, time };
    if bad
        || pairs
            .get("need")
            .is_some_and(|n| n.split(',').any(|k| !KNOWN.contains(&k)))
    {
        return rejected;
    }
    match (pairs.get("argv"), pairs.get("clear")) {
        (None, Some(&"1")) => Parsed::Clear { pid, time },
        (Some(argv), None) => {
            let argv: Option<Vec<String>> = argv.split(',').map(text).collect();
            // Some(None): there's a cwd, and it isn't an absolute path.
            let cwd = pairs
                .get("cwd")
                .map(|c| text(c).filter(|c| c.starts_with('/')));
            match (argv, cwd) {
                (_, Some(None)) => rejected,
                (Some(argv), cwd)
                    if argv.len() <= MAX_ARGV
                        && !argv[0].is_empty()
                        && !argv[0].starts_with('-')
                        && (!argv[0].contains('/') || argv[0].starts_with('/')) =>
                {
                    Parsed::Report {
                        pid,
                        time,
                        argv,
                        cwd: cwd.flatten(),
                    }
                }
                _ => rejected,
            }
        }
        _ => rejected,
    }
}

/// One base64 element as text: valid UTF-8 with no control characters.
/// Padding is optional, but when there is some it has to be all of it.
fn text(b64: &str) -> Option<String> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
    let engine = if b64.ends_with('=') {
        STANDARD
    } else {
        STANDARD_NO_PAD
    };
    let s = String::from_utf8(engine.decode(b64).ok()?).ok()?;
    (!s.chars().any(char::is_control)).then_some(s)
}

/// The process a record belongs to.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Owner {
    pub pid: u32,
    /// When it started (`abromux_platform::process::info`), so a reused
    /// pid isn't taken for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
}

/// What an accepted report leaves.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Record {
    /// None for a command this terminal resumed itself, until the program
    /// reports again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Owner>,
    /// How far below the shell its owner is (the shell's children are 1),
    /// so an ancestor's record wins over a descendant's.
    pub depth: u32,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// `cwd` is the report's own (`cwd` key), not the owner's folder read
    /// from the OS: if it's gone the record isn't resumed, where another
    /// folder that's gone just means the shell's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cwd_reported: bool,
    /// Its owner ran under the terminal's coding agent, which then is the
    /// one to resume (an ancestor wins over its descendants).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub under_agent: bool,
    /// When its owner was first seen gone.
    #[serde(skip)]
    gone: Option<Instant>,
}

/// What the terminal knows of a reporting process: None from the check
/// means it isn't one to take reports from (gone, another user's, not
/// under this terminal's shell).
pub struct Checked {
    pub started: Option<u64>,
    pub depth: u32,
    /// Its folder now, for a report without `cwd`.
    pub cwd: Option<String>,
    /// It runs under the terminal's coding agent.
    pub under_agent: bool,
}

/// A terminal's records.
#[derive(Default, Debug)]
pub struct Records {
    list: Vec<Record>,
    /// The newest time taken from each pid: older reports are replays.
    last: HashMap<u32, u64>,
}

impl Records {
    /// Records saved with a terminal that kept running.
    pub fn load(list: Vec<Record>) -> Self {
        Records {
            list,
            last: HashMap::new(),
        }
    }

    pub fn list(&self) -> &[Record] {
        &self.list
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The command this terminal ran on restore, owned by whichever
    /// program reports from the shell's foreground next.
    pub fn restored(&mut self, r: &Restart) {
        self.list.push(Record {
            owner: None,
            depth: 1,
            argv: r.argv.clone(),
            cwd: r.cwd.clone(),
            cwd_reported: r.cwd_required,
            under_agent: false,
            gone: None,
        });
    }

    /// The command the terminal resumed has ended (the shell integration's
    /// mark after it), and nothing took its record over.
    pub fn restored_ended(&mut self) -> bool {
        let n = self.list.len();
        self.list.retain(|r| r.owner.is_some());
        self.list.len() != n
    }

    /// Take a parsed report; `check` looks the pid up. True if the records
    /// changed.
    pub fn apply(&mut self, p: Parsed, check: impl FnOnce(u32) -> Option<Checked>) -> bool {
        let (pid, time) = match &p {
            Parsed::Ignored => return false,
            Parsed::Rejected { pid, time }
            | Parsed::Report { pid, time, .. }
            | Parsed::Clear { pid, time } => (*pid, *time),
        };
        if self.last.get(&pid).is_some_and(|&t| time < t) {
            return false;
        }
        let n = self.list.len();
        let Some(proc) = check(pid) else {
            // Signed, but not from a process to take reports from: it
            // counts as rejected, so whatever that pid had goes.
            self.list
                .retain(|r| r.owner.as_ref().is_none_or(|o| o.pid != pid));
            return self.list.len() != n;
        };
        self.last.insert(pid, time);
        // The program resumed here (a child of the shell) speaks for itself
        // from now on.
        let takes_restored = proc.depth <= 1;
        self.list.retain(|r| match &r.owner {
            Some(o) => o.pid != pid,
            None => !takes_restored,
        });
        if let Parsed::Report { argv, cwd, .. } = p {
            self.list.push(Record {
                owner: Some(Owner {
                    pid,
                    started: proc.started,
                }),
                depth: proc.depth,
                argv,
                cwd_reported: cwd.is_some(),
                cwd: cwd.or(proc.cwd),
                under_agent: proc.under_agent,
                gone: None,
            });
            return true;
        }
        self.list.len() != n
    }

    /// Drop records whose owner has been gone for longer than [`GRACE`];
    /// `alive` checks one. True if any went.
    pub fn check_owners(&mut self, now: Instant, alive: impl Fn(&Owner) -> bool) -> bool {
        let n = self.list.len();
        self.list.retain_mut(|r| {
            let Some(o) = &r.owner else { return true };
            if alive(o) {
                r.gone = None;
                return true;
            }
            let since = *r.gone.get_or_insert(now);
            now.duration_since(since) < GRACE
        });
        self.list.len() != n
    }

    /// The record to resume: the owner nearest the shell, the newest of
    /// those.
    pub fn best(&self) -> Option<&Record> {
        self.list.iter().rev().min_by_key(|r| r.depth)
    }
}

/// A command to run in a new shell, from a record or an agent's session.
#[derive(Debug, Clone, PartialEq)]
pub struct Restart {
    pub argv: Vec<String>,
    /// The folder to run it in. The shell integration doesn't run it when
    /// this is gone, so one that isn't required is only kept if it's there.
    pub cwd: Option<String>,
    /// The program said it needs `cwd` (or an agent's session belongs to it).
    pub cwd_required: bool,
}

impl From<&Record> for Restart {
    /// Made at restore: a folder the program didn't ask for that's gone by
    /// now leaves the new shell's.
    fn from(r: &Record) -> Self {
        let cwd = r
            .cwd
            .clone()
            .filter(|c| r.cwd_reported || std::path::Path::new(c).is_dir());
        Restart {
            argv: r.argv.clone(),
            cwd,
            cwd_required: r.cwd_reported,
        }
    }
}

impl Restart {
    /// The one-time file the shell integration reads: a format version, the
    /// folder (empty for none), then each argv element, each ending in NUL.
    pub fn file_bytes(&self) -> Vec<u8> {
        let mut out = b"1\0".to_vec();
        for s in std::iter::once(self.cwd.as_deref().unwrap_or(""))
            .chain(self.argv.iter().map(String::as_str))
        {
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }
        out
    }
}

/// What a terminal's session entry keeps.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Saved {
    /// The token its shell has, for a terminal that keeps running (held):
    /// the next launch takes its reports with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub records: Vec<Record>,
}

impl Saved {
    /// The record to resume, as [`Records::best`].
    pub fn best(&self) -> Option<&Record> {
        self.records.iter().rev().min_by_key(|r| r.depth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "dummy-token-replace-me";

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    fn signed(body: &str) -> Vec<u8> {
        format!("{body}:mac={}", mac(body.as_bytes(), TOKEN)).into_bytes()
    }

    fn here(depth: u32) -> impl FnOnce(u32) -> Option<Checked> {
        move |_| {
            Some(Checked {
                started: Some(7),
                depth,
                cwd: Some("/os".into()),
                under_agent: false,
            })
        }
    }

    #[test]
    fn the_specs_clear_vector() {
        let body = b"pid=74877:time=1795543200:clear=1";
        assert_eq!(
            mac(body, TOKEN),
            "c30603369f98fef581a4098be889175b0c74f0641e49f8b25f4326cb38f97c93"
        );
        let wire = b"pid=74877:time=1795543200:clear=1:mac=c30603369f98fef581a4098be889175b0c74f0641e49f8b25f4326cb38f97c93";
        assert_eq!(
            parse(wire, TOKEN, 1795543200),
            Parsed::Clear {
                pid: 74877,
                time: 1795543200
            }
        );
    }

    #[test]
    fn the_specs_neovim_vector() {
        let wire = b"pid=74877:time=1795543200:argv=bnZpbQ==,LVM=,L2hvbWUvdXNlci8ubG9jYWwvc3RhdGUvbnZpbS9zZXNzaW9ucy84ZjJjMWQudmlt:mac=9dbb99bfb856749a87563839d7ce9062818d78cf25909db93f309e0cd3b1909b";
        assert_eq!(
            parse(wire, TOKEN, 1795543200),
            Parsed::Report {
                pid: 74877,
                time: 1795543200,
                argv: vec![
                    "nvim".into(),
                    "-S".into(),
                    "/home/user/.local/state/nvim/sessions/8f2c1d.vim".into()
                ],
                cwd: None,
            }
        );
    }

    #[test]
    fn reads_a_report() {
        let body = format!(
            "pid=12:time=1000:argv={},{},{}:cwd={}",
            b64("nvim"),
            b64("-S"),
            b64("/s/a b.vim"),
            b64("/home/u")
        );
        assert_eq!(
            parse(&signed(&body), TOKEN, 1010),
            Parsed::Report {
                pid: 12,
                time: 1000,
                argv: vec!["nvim".into(), "-S".into(), "/s/a b.vim".into()],
                cwd: Some("/home/u".into()),
            }
        );
        // Padding is optional; empty elements are fine after the first.
        let body = format!("pid=12:time=1000:argv=bnZpbQ,,{}", b64("x"));
        assert!(matches!(
            parse(&signed(&body), TOKEN, 1000),
            Parsed::Report { argv, .. } if argv == ["nvim", "", "x"]
        ));
    }

    #[test]
    fn ignores_what_the_token_didnt_sign() {
        let body = format!("pid=12:time=1000:argv={}", b64("nvim"));
        let good = signed(&body);
        assert_eq!(
            parse(&good, "another-token-entirely", 1000),
            Parsed::Ignored
        );
        assert_eq!(parse(body.as_bytes(), TOKEN, 1000), Parsed::Ignored);
        // Stale or from the future.
        assert_eq!(parse(&good, TOKEN, 1061), Parsed::Ignored);
        assert_eq!(parse(&good, TOKEN, 939), Parsed::Ignored);
        // mac not last.
        let mut moved = good.clone();
        moved.extend_from_slice(b":cwd=Lw==");
        assert_eq!(parse(&moved, TOKEN, 1000), Parsed::Ignored);
        assert_eq!(parse(b"?", TOKEN, 1000), Parsed::Ignored);
        // Signed but no time.
        assert_eq!(
            parse(&signed("pid=12:clear=1"), TOKEN, 1000),
            Parsed::Ignored
        );
    }

    #[test]
    fn rejects_what_the_token_signed_wrong() {
        let rejected = Parsed::Rejected {
            pid: 12,
            time: 1000,
        };
        for body in [
            "pid=12:time=1000".to_string(),
            "pid=12:time=1000:clear=1:clear=1".into(),
            format!("pid=12:time=1000:clear=1:argv={}", b64("x")),
            format!("pid=12:time=1000:argv={}", b64("bin/x")),
            format!("pid=12:time=1000:argv={},{}", b64("x"), b64("a\u{1b}b")),
            format!("pid=12:time=1000:argv={}:cwd={}", b64("x"), b64("rel")),
            format!("pid=12:time=1000:argv={}:need=frob", b64("x")),
            "pid=12:time=1000:argv=:".into(),
            "pid=12:time=1000:argv=!!".into(),
        ] {
            assert_eq!(parse(&signed(&body), TOKEN, 1000), rejected, "{body}");
        }
        let body = format!("pid=12:time=1000:argv={}:need=cwd,argv:zz=1", b64("x"));
        assert!(matches!(
            parse(&signed(&body), TOKEN, 1000),
            Parsed::Report { .. }
        ));
    }

    fn report(pid: u32, time: u64, cmd: &str) -> Parsed {
        Parsed::Report {
            pid,
            time,
            argv: vec![cmd.into()],
            cwd: None,
        }
    }

    #[test]
    fn keeps_one_record_per_owner_and_resumes_the_shallowest() {
        let mut r = Records::default();
        assert!(r.apply(report(20, 5, "lazygit"), here(2)));
        assert!(r.apply(report(10, 5, "nvim"), here(1)));
        assert!(r.apply(report(10, 6, "nvim2"), here(1)));
        assert_eq!(r.list().len(), 2);
        let best = r.best().unwrap();
        assert_eq!(best.argv, ["nvim2"]);
        assert_eq!(best.cwd.as_deref(), Some("/os"));
        // A replayed older report is ignored.
        assert!(!r.apply(Parsed::Clear { pid: 10, time: 5 }, here(1)));
        assert!(r.apply(Parsed::Clear { pid: 10, time: 7 }, here(1)));
        assert_eq!(r.best().unwrap().argv, ["lazygit"]);
        assert!(r.apply(Parsed::Rejected { pid: 20, time: 8 }, here(2)));
        assert!(r.is_empty());
        // A process the check refuses changes nothing.
        assert!(!r.apply(report(30, 9, "x"), |_| None));
    }

    #[test]
    fn a_resumed_command_is_taken_over_by_the_program_reporting() {
        let mut r = Records::default();
        r.restored(&Restart {
            argv: vec!["nvim".into()],
            cwd: None,
            cwd_required: false,
        });
        // Something deeper (lazygit inside nvim) doesn't take it.
        r.apply(report(20, 5, "lazygit"), here(2));
        assert_eq!(r.list().len(), 2);
        r.apply(report(10, 5, "nvim"), here(1));
        assert_eq!(r.list().len(), 2);
        assert!(r.list().iter().all(|x| x.owner.is_some()));
        assert!(!r.restored_ended());
    }

    #[test]
    fn a_gone_owner_keeps_its_record_for_the_grace() {
        let mut r = Records::default();
        r.apply(report(10, 5, "nvim"), here(1));
        let t = Instant::now();
        assert!(!r.check_owners(t, |_| false));
        assert!(!r.check_owners(t + GRACE / 2, |_| false));
        assert_eq!(r.list().len(), 1);
        assert!(r.check_owners(t + GRACE, |_| false));
        assert!(r.is_empty());
    }

    #[test]
    fn rejects_options_short_padding_and_bytes_that_arent_text() {
        let rejected = Parsed::Rejected {
            pid: 12,
            time: 1000,
        };
        for body in [
            format!("pid=12:time=1000:argv={}", b64("-x")),
            // "nvim" needs two `=`.
            "pid=12:time=1000:argv=bnZpbQ=".to_string(),
            format!("pid=12:time=1000:argv={},LVM==", b64("x")),
        ] {
            assert_eq!(parse(&signed(&body), TOKEN, 1000), rejected, "{body}");
        }
        let mut body = b"pid=12:time=1000:argv=\xff".to_vec();
        let m = mac(&body, TOKEN);
        body.extend_from_slice(format!(":mac={m}").as_bytes());
        assert_eq!(parse(&body, TOKEN, 1000), rejected);
        // A later element may still go without padding, or start with `-`.
        let body = format!("pid=12:time=1000:argv={},LVM,LVM=", b64("x"));
        assert!(matches!(
            parse(&signed(&body), TOKEN, 1000),
            Parsed::Report { argv, .. } if argv == ["x", "-S", "-S"]
        ));
    }

    #[test]
    fn a_process_the_check_refuses_loses_its_records() {
        let mut r = Records::default();
        r.apply(report(10, 5, "nvim"), here(1));
        r.apply(report(20, 5, "lazygit"), here(2));
        assert!(r.apply(report(10, 6, "x"), |_| None));
        assert_eq!(r.best().unwrap().argv, ["lazygit"]);
    }

    #[test]
    fn only_a_reported_folder_is_required() {
        let mut r = Records::default();
        let gone = "/no/such/folder/abromux";
        r.apply(report(10, 5, "nvim"), |_| {
            Some(Checked {
                started: None,
                depth: 1,
                cwd: Some(gone.into()),
                under_agent: false,
            })
        });
        // The OS's folder for it is gone: the shell's instead.
        let x = Restart::from(r.best().unwrap());
        assert_eq!((x.cwd, x.cwd_required), (None, false));
        let reported = Parsed::Report {
            pid: 10,
            time: 6,
            argv: vec!["nvim".into()],
            cwd: Some(gone.into()),
        };
        r.apply(reported, here(1));
        // It asked for that one: kept, so the shell doesn't run it.
        let x = Restart::from(r.best().unwrap());
        assert_eq!((x.cwd.as_deref(), x.cwd_required), (Some(gone), true));
        assert_eq!(
            Restart::from(&Record {
                cwd_reported: false,
                cwd: Some("/".into()),
                ..r.best().unwrap().clone()
            })
            .cwd
            .as_deref(),
            Some("/")
        );
    }

    #[test]
    fn the_file_the_shell_reads() {
        let r = Restart {
            argv: vec!["nvim".into(), "".into(), "a b".into()],
            cwd: None,
            cwd_required: false,
        };
        assert_eq!(r.file_bytes(), b"1\0\0nvim\0\0a b\0");
    }

    #[test]
    fn tokens_are_fresh_and_in_the_alphabet() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        );
    }
}
