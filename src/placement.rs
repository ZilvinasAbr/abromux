//! Where the window goes on the screens there are. The window's frame is
//! remembered for each arrangement of screens it has been on (the session
//! keeps them), so unplugging a monitor and plugging it back in, or quitting
//! on one desk and launching at another, puts it back where it was there.
//! On screens it hasn't been on, it's fitted onto the one it's mostly on,
//! instead of left hanging off the edge or bigger than the screen.

use abromux_core::session::Placement;
use abromux_platform::window::{self as os, Frame, Screen};
use gpui::Window;

/// How many arrangements are remembered.
const KEEP: usize = 8;

#[derive(Default)]
pub struct Placements {
    /// Oldest first.
    list: Vec<Placement>,
    /// The arrangement the window was last seen on.
    screens: String,
}

/// A key for an arrangement of screens: each one's frame, as AppKit lays
/// them out (a monitor moved in Displays settings counts as another).
fn key(screens: &[Screen]) -> String {
    let mut parts: Vec<String> = screens
        .iter()
        .map(|s| {
            let f = s.frame;
            format!("{},{},{}x{}", f.x, f.y, f.width, f.height)
        })
        .collect();
    parts.sort();
    parts.join(";")
}

fn frame_of(p: &Placement) -> Frame {
    Frame {
        x: p.x,
        y: p.y,
        width: p.width,
        height: p.height,
    }
}

fn overlap(a: Frame, b: Frame) -> f64 {
    let w = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let h = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    w.max(0.) * h.max(0.)
}

/// `f` moved (and shrunk, if it must) to lie inside the visible part of the
/// screen it's mostly on; the primary screen when it's on none.
fn fit(f: Frame, screens: &[Screen]) -> Frame {
    let Some(on) = screens
        .iter()
        .map(|s| s.visible)
        .filter(|v| overlap(f, *v) > 0.)
        .max_by(|a, b| overlap(f, *a).total_cmp(&overlap(f, *b)))
        .or_else(|| screens.first().map(|s| s.visible))
    else {
        return f;
    };
    let width = f.width.min(on.width);
    let height = f.height.min(on.height);
    Frame {
        x: f.x.clamp(on.x, on.x + on.width - width),
        y: f.y.clamp(on.y, on.y + on.height - height),
        width,
        height,
    }
}

impl Placements {
    pub fn new(list: Vec<Placement>) -> Self {
        Self {
            list,
            screens: key(&os::screens()),
        }
    }

    /// For the session file.
    pub fn list(&self) -> Vec<Placement> {
        self.list.clone()
    }

    fn find(&self, screens: &str) -> Option<Frame> {
        self.list
            .iter()
            .rev()
            .find(|p| p.screens == screens)
            .map(frame_of)
    }

    fn remember(&mut self, screens: &str, f: Frame) {
        self.list.retain(|p| p.screens != screens);
        self.list.push(Placement {
            screens: screens.to_string(),
            x: f.x,
            y: f.y,
            width: f.width,
            height: f.height,
        });
        let extra = self.list.len().saturating_sub(KEEP);
        self.list.drain(..extra);
    }

    /// The window just opened: where it was on these screens, else where it
    /// last was, fitted onto them.
    pub fn place(&mut self, window: &Window) {
        let screens = os::screens();
        if screens.is_empty() {
            return;
        }
        self.screens = key(&screens);
        let Some(to) = self
            .find(&self.screens)
            .or_else(|| self.list.last().map(frame_of))
        else {
            return;
        };
        let to = fit(to, &screens);
        if os::set_frame(window, to).is_ok() {
            let k = self.screens.clone();
            self.remember(&k, to);
        }
    }

    /// The window moved, or the screens may have changed: remember where
    /// it is, or on another arrangement, put it where it was on that one.
    pub fn sync(&mut self, window: &Window) {
        let screens = os::screens();
        if screens.is_empty() {
            return;
        }
        // Full screen (None) isn't a place to keep; macOS keeps it on its
        // screen and it comes back to its own frame.
        let Ok(Some(now)) = os::frame(window) else {
            return;
        };
        let k = key(&screens);
        if k == self.screens {
            self.remember(&k, now);
            return;
        }
        self.screens = k.clone();
        let to = fit(self.find(&k).unwrap_or(now), &screens);
        if to != now {
            let _ = os::set_frame(window, to);
        }
        self.remember(&k, to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, width: f64, height: f64) -> Frame {
        Frame {
            x,
            y,
            width,
            height,
        }
    }

    fn screen(f: Frame) -> Screen {
        Screen {
            frame: f,
            visible: r(f.x, f.y + 30., f.width, f.height - 30.),
        }
    }

    #[test]
    fn fits_onto_the_screen_it_is_mostly_on() {
        let laptop = screen(r(0., 0., 1512., 982.));
        let monitor = screen(r(1512., -300., 2560., 1440.));
        // On the monitor, which is gone: onto the laptop, shrunk to fit.
        let big = r(1600., -200., 2400., 1300.);
        assert_eq!(fit(big, &[laptop]), r(0., 30., 1512., 952.));
        // Mostly on the monitor: kept there, pulled in from the edge.
        let edge = r(3800., 100., 800., 600.);
        assert_eq!(fit(edge, &[laptop, monitor]), r(3272., 100., 800., 600.));
        // Inside already: unchanged.
        let inside = r(100., 100., 800., 600.);
        assert_eq!(fit(inside, &[laptop, monitor]), inside);
    }

    #[test]
    fn remembers_one_frame_per_arrangement() {
        let mut p = Placements::default();
        for i in 0..KEEP + 2 {
            p.remember("a", r(i as f64, 0., 10., 10.));
            p.remember(&format!("s{i}"), r(0., 0., 10., 10.));
        }
        assert_eq!(p.list.len(), KEEP);
        assert_eq!(p.find("a"), Some(r((KEEP + 1) as f64, 0., 10., 10.)));
        assert_eq!(p.find("s0"), None);
    }
}
