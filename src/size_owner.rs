//! Who sets a terminal's size when the Mac and phones show it at once.
//!
//! A terminal has one size (its PTY's), so one side has to win. As in Rex
//! (`session.own_block`), the terminal follows its owner. The owner is the
//! side used last: a phone takes the size when it opens the terminal or
//! types in it, and the Mac takes it back when someone types in its pane.
//!
//! Claims stack, the owner on top. A phone's claim lasts while it has the
//! terminal open (its `surface.stream`s); when the last one ends, the
//! claim goes and the next one down owns the size, in the end the Mac,
//! whose claim never goes.

use std::collections::HashMap;

/// The client that owns a terminal's size; `None` is the Mac.
pub type Owner<'a> = Option<&'a str>;

/// A size a client may ask for: the PTY's limits, not the screen's.
pub fn clamp(cols: u64, rows: u64) -> (u16, u16) {
    (cols.clamp(2, 1000) as u16, rows.clamp(1, 500) as u16)
}

#[derive(Clone, Debug, PartialEq)]
enum Claim {
    /// The Mac pane: its layout's size.
    Local,
    Client(String, (u16, u16)),
}

#[derive(Debug)]
pub struct Sizing {
    /// Oldest first; the last owns the size. The Mac's is always in it.
    claims: Vec<Claim>,
    /// Clients that have the terminal open, by how many streams.
    watching: HashMap<String, usize>,
}

impl Default for Sizing {
    fn default() -> Self {
        Self {
            claims: vec![Claim::Local],
            watching: HashMap::new(),
        }
    }
}

impl Sizing {
    /// Whose size it has, and the size if it isn't the Mac's.
    fn state(&self) -> (Option<String>, Option<(u16, u16)>) {
        match self.claims.last() {
            Some(Claim::Client(c, size)) => (Some(c.clone()), Some(*size)),
            _ => (None, None),
        }
    }

    fn changes(&mut self, f: impl FnOnce(&mut Self)) -> bool {
        let before = self.state();
        f(self);
        self.state() != before
    }

    fn find(&self, client: &str) -> Option<usize> {
        self.claims
            .iter()
            .position(|c| matches!(c, Claim::Client(n, _) if n == client))
    }

    pub fn owner(&self) -> Owner<'_> {
        match self.claims.last() {
            Some(Claim::Client(c, _)) => Some(c),
            _ => None,
        }
    }

    /// The size the owner asks for; `None` while the Mac's layout decides.
    pub fn size(&self) -> Option<(u16, u16)> {
        self.state().1
    }

    /// `client` opened a stream of the terminal.
    pub fn watch(&mut self, client: &str) {
        *self.watching.entry(client.to_string()).or_default() += 1;
    }

    /// One of `client`'s streams ended; with its last, its claim goes.
    /// True if the owner or size changed.
    pub fn unwatch(&mut self, client: &str) -> bool {
        self.changes(|s| {
            let Some(n) = s.watching.get_mut(client) else {
                return;
            };
            *n -= 1;
            if *n == 0 {
                s.watching.remove(client);
                if let Some(i) = s.find(client) {
                    s.claims.remove(i);
                }
            }
        })
    }

    /// `client` wants the terminal `size`. With `take` it owns it now;
    /// without, a claim it has keeps its place (a phone turned sideways
    /// while the Mac has the size). Err if it hasn't the terminal open.
    pub fn claim(&mut self, client: &str, size: (u16, u16), take: bool) -> Result<bool, NotOpen> {
        if !self.watching.contains_key(client) {
            return Err(NotOpen);
        }
        Ok(self.changes(|s| {
            let claim = Claim::Client(client.to_string(), size);
            match s.find(client) {
                Some(i) if !take => s.claims[i] = claim,
                Some(i) => {
                    s.claims.remove(i);
                    s.claims.push(claim);
                }
                // A first claim without `take` waits under the Mac's.
                None if !take => s.claims.insert(0, claim),
                None => s.claims.push(claim),
            }
        }))
    }

    /// `client` typed here: it owns the size again, if it has a claim.
    pub fn retake(&mut self, client: &str) -> bool {
        self.changes(|s| {
            if let Some(i) = s.find(client) {
                let c = s.claims.remove(i);
                s.claims.push(c);
            }
        })
    }

    /// Someone typed in the Mac pane: the Mac's layout sizes it again.
    pub fn take_local(&mut self) -> bool {
        self.changes(|s| {
            s.claims.retain(|c| *c != Claim::Local);
            s.claims.push(Claim::Local);
        })
    }
}

#[derive(Debug, PartialEq)]
pub struct NotOpen;

impl std::fmt::Display for NotOpen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("open the terminal with surface.stream first")
    }
}

impl std::error::Error for NotOpen {}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(s: &mut Sizing, c: &str, size: (u16, u16)) -> bool {
        s.watch(c);
        s.claim(c, size, true).unwrap()
    }

    #[test]
    fn the_mac_owns_it_until_a_phone_opens_it() {
        let mut s = Sizing::default();
        assert_eq!((s.owner(), s.size()), (None, None));
        assert!(open(&mut s, "phone:1", (40, 30)));
        assert_eq!((s.owner(), s.size()), (Some("phone:1"), Some((40, 30))));
    }

    #[test]
    fn typing_on_the_mac_takes_it_back_and_on_the_phone_again() {
        let mut s = Sizing::default();
        open(&mut s, "phone:1", (40, 30));
        assert!(s.take_local());
        assert_eq!(s.owner(), None);
        assert!(!s.take_local(), "already the Mac's");
        assert!(s.retake("phone:1"));
        assert_eq!(s.size(), Some((40, 30)));
        assert!(!s.retake("phone:2"), "no claim, nothing to retake");
    }

    #[test]
    fn a_resize_without_take_keeps_its_place() {
        let mut s = Sizing::default();
        open(&mut s, "phone:1", (40, 30));
        s.take_local();
        assert_eq!(s.claim("phone:1", (70, 20), false), Ok(false));
        assert_eq!(s.owner(), None);
        assert!(s.retake("phone:1"));
        assert_eq!(s.size(), Some((70, 20)), "with the new size");
        assert_eq!(s.claim("phone:1", (71, 20), false), Ok(true));
    }

    #[test]
    fn a_first_claim_without_take_waits_under_the_mac() {
        let mut s = Sizing::default();
        s.watch("phone:1");
        assert_eq!(s.claim("phone:1", (40, 30), false), Ok(false));
        assert_eq!(s.owner(), None);
        assert!(s.retake("phone:1"));
    }

    #[test]
    fn a_claim_needs_the_terminal_open() {
        let mut s = Sizing::default();
        assert_eq!(s.claim("phone:1", (40, 30), true), Err(NotOpen));
    }

    #[test]
    fn closing_it_hands_the_size_down_the_stack() {
        let mut s = Sizing::default();
        open(&mut s, "phone:1", (40, 30));
        open(&mut s, "phone:2", (50, 25));
        assert_eq!(s.owner(), Some("phone:2"));
        assert!(s.unwatch("phone:2"));
        assert_eq!((s.owner(), s.size()), (Some("phone:1"), Some((40, 30))));
        assert!(s.unwatch("phone:1"));
        assert_eq!(s.owner(), None);
        assert!(!s.unwatch("phone:1"), "not watching any more");
    }

    #[test]
    fn a_restarted_stream_keeps_the_claim() {
        // The phone opens the new stream before the old one's end lands.
        let mut s = Sizing::default();
        open(&mut s, "phone:1", (40, 30));
        s.watch("phone:1");
        assert!(!s.unwatch("phone:1"));
        assert_eq!(s.owner(), Some("phone:1"));
    }

    #[test]
    fn a_closed_owner_below_the_top_changes_nothing() {
        let mut s = Sizing::default();
        open(&mut s, "phone:1", (40, 30));
        s.take_local();
        assert!(!s.unwatch("phone:1"));
        assert!(!s.retake("phone:1"), "its claim went");
    }

    #[test]
    fn sizes_stay_in_the_ptys_limits() {
        assert_eq!(clamp(0, 0), (2, 1));
        assert_eq!(clamp(90_000, 90_000), (1000, 500));
        assert_eq!(clamp(80, 24), (80, 24));
    }
}
