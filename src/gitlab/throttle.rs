//! Slowing down for GitLab's rate limits.
//!
//! One factor, `slow`, multiplies every polling interval. A 429 pauses all
//! requests until its `Retry-After` and doubles the factor (to ×32 at most);
//! running low on `RateLimit-Remaining` raises it before that happens. After
//! five minutes without either, it shrinks by a fifth, back down to ×1:
//! quick to back off, slow to speed up again, so it doesn't swing.
//!
//! The higher it is, the less runs at all (`allows`): diffs and details in
//! the background stop first, the merge request you're looking at last.

use std::time::{Duration, Instant};

const MAX_SLOW: f64 = 32.;
const CALM: Duration = Duration::from_secs(300);
/// Below this share of the limit left, slow down before GitLab says to.
const LOW: f64 = 0.2;
/// Raising for a low remaining count at most this often.
const LOW_EVERY: Duration = Duration::from_secs(30);
/// A 429 without `Retry-After` waits this long.
const DEFAULT_PAUSE: Duration = Duration::from_secs(60);

/// How much a job matters, most first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Prio {
    /// Just opened, or Refresh pressed.
    Opened = 0,
    OnScreen = 1,
    Running = 2,
    Mine = 3,
    Recent = 4,
    Background = 5,
}

#[derive(Debug)]
pub struct Throttle {
    pub slow: f64,
    pub paused_until: Option<Instant>,
    /// The last time it was raised.
    raised: Option<Instant>,
    /// The last time it was lowered (or raised).
    eased: Instant,
    pub last_remaining: Option<(u64, u64)>,
}

impl Default for Throttle {
    fn default() -> Self {
        Self {
            slow: 1.,
            paused_until: None,
            raised: None,
            eased: Instant::now(),
            last_remaining: None,
        }
    }
}

impl Throttle {
    /// GitLab said 429.
    pub fn throttled(&mut self, retry_after: Option<Duration>, now: Instant) {
        self.paused_until = Some(now + retry_after.unwrap_or(DEFAULT_PAUSE));
        self.raise(2., now);
    }

    /// A response's `RateLimit-Remaining` / `RateLimit-Limit`.
    pub fn remaining(&mut self, remaining: u64, limit: u64, now: Instant) {
        self.last_remaining = Some((remaining, limit));
        if limit > 0
            && (remaining as f64) < LOW * limit as f64
            && self.raised.is_none_or(|t| now.duration_since(t) >= LOW_EVERY)
        {
            self.raise(1.5, now);
        }
    }

    fn raise(&mut self, by: f64, now: Instant) {
        self.slow = (self.slow * by).min(MAX_SLOW);
        self.raised = Some(now);
        self.eased = now;
    }

    /// Ease off after a calm spell; call it now and then.
    pub fn tick(&mut self, now: Instant) {
        if self.paused_until.is_some_and(|t| now >= t) {
            self.paused_until = None;
        }
        if self.slow > 1. && now.duration_since(self.eased) >= CALM {
            self.slow = (self.slow * 0.8).max(1.);
            self.eased = now;
        }
    }

    pub fn paused(&self, now: Instant) -> Option<Duration> {
        self.paused_until
            .filter(|t| *t > now)
            .map(|t| t.duration_since(now))
    }

    /// Whether a job of `prio` may run now.
    pub fn allows(&self, prio: Prio, now: Instant) -> bool {
        if self.paused(now).is_some() {
            return false;
        }
        let cutoff = if self.slow < 4. {
            Prio::Background
        } else if self.slow < 8. {
            Prio::Recent
        } else if self.slow < 16. {
            Prio::Mine
        } else {
            Prio::OnScreen
        };
        prio <= cutoff
    }

    /// `secs` stretched by the slowdown.
    pub fn interval(&self, secs: u32) -> Duration {
        Duration::from_secs_f64(secs.max(5) as f64 * self.slow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_off_and_recovers() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        t.throttled(Some(Duration::from_secs(40)), t0);
        assert_eq!(t.slow, 2.);
        assert!(!t.allows(Prio::Opened, t0));
        assert_eq!(t.paused(t0), Some(Duration::from_secs(40)));
        t.tick(t0 + Duration::from_secs(41));
        assert!(t.allows(Prio::Opened, t0 + Duration::from_secs(41)));
        for _ in 0..10 {
            t.throttled(None, t0);
        }
        assert_eq!(t.slow, MAX_SLOW);
        assert_eq!(t.interval(20), Duration::from_secs(640));
        // Only what's on screen runs this slow.
        let later = t0 + Duration::from_secs(61);
        t.tick(later);
        assert!(t.allows(Prio::OnScreen, later));
        assert!(!t.allows(Prio::Mine, later));
        // Calm spells bring it down step by step.
        let mut now = later;
        for _ in 0..40 {
            now += CALM;
            t.tick(now);
        }
        assert_eq!(t.slow, 1.);
        assert!(t.allows(Prio::Background, now));
    }

    #[test]
    fn slows_before_running_out() {
        let t0 = Instant::now();
        let mut t = Throttle::default();
        t.remaining(500, 2000, t0);
        assert_eq!(t.slow, 1.);
        t.remaining(100, 2000, t0);
        assert_eq!(t.slow, 1.5);
        // Not again right away.
        t.remaining(90, 2000, t0 + Duration::from_secs(1));
        assert_eq!(t.slow, 1.5);
        t.remaining(80, 2000, t0 + LOW_EVERY);
        assert_eq!(t.slow, 2.25);
    }
}
