//! Box-drawing (U+2500–257F) and block-element (U+2580–259F) characters,
//! drawn as shapes filling the whole cell rather than taken from the font, as
//! Ghostty's sprite font does. Font glyphs only span the font's ascent and
//! descent, so with any extra line height they leave gaps between rows.

use gpui::*;

/// Style of one arm of a box-drawing character.
#[derive(Clone, Copy, PartialEq)]
enum Line {
    None,
    Light,
    Heavy,
    Double,
}

pub fn covers(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{259f}')
}

/// Paints `c` over `cell`. `bg` is the cell's background, used to open the
/// gap inside double lines.
pub fn paint(
    c: char,
    cell: Bounds<Pixels>,
    font_size: Pixels,
    fg: Hsla,
    bg: Hsla,
    window: &mut Window,
) {
    let scale = window.scale_factor();
    let snap = |v: Pixels| (f32::from(v) * scale).round() / scale;
    let mut cv = Canvas {
        x0: snap(cell.origin.x),
        y0: snap(cell.origin.y),
        x1: snap(cell.origin.x + cell.size.width),
        y1: snap(cell.origin.y + cell.size.height),
        scale,
        light: (f32::from(font_size) * scale / 14.).round().max(1.) / scale,
        fg,
        bg,
        window,
    };
    let code = c as u32;
    if let Some(arms) = arms(code) {
        return cv.lines(arms);
    }
    let t = cv.light;
    match code {
        // ┄ ┅ ┆ ┇ ┈ ┉ ┊ ┋
        0x2504..=0x250b => {
            let i = code - 0x2504;
            let n = if i < 4 { 3 } else { 4 };
            cv.dashes(i % 4 < 2, n, if i % 2 == 0 { t } else { 2. * t });
        }
        // ╌ ╍ ╎ ╏
        0x254c..=0x254f => {
            let i = code - 0x254c;
            cv.dashes(i < 2, 2, if i % 2 == 0 { t } else { 2. * t });
        }
        // ╭ ╮ ╯ ╰
        0x256d => cv.rounded(cv.x1, cv.y1),
        0x256e => cv.rounded(cv.x0, cv.y1),
        0x256f => cv.rounded(cv.x0, cv.y0),
        0x2570 => cv.rounded(cv.x1, cv.y0),
        // ╱ ╲ ╳
        0x2571 => cv.diagonal(false),
        0x2572 => cv.diagonal(true),
        0x2573 => {
            cv.diagonal(false);
            cv.diagonal(true);
        }
        0x2580 => cv.block(0., 0., 1., 0.5),
        // ▁ ▂ ▃ ▄ ▅ ▆ ▇ █
        0x2581..=0x2588 => cv.block(0., 1. - (code - 0x2580) as f32 / 8., 1., 1.),
        // ▉ ▊ ▋ ▌ ▍ ▎ ▏
        0x2589..=0x258f => cv.block(0., 0., (0x2590 - code) as f32 / 8., 1.),
        0x2590 => cv.block(0.5, 0., 1., 1.),
        // ░ ▒ ▓
        0x2591..=0x2593 => {
            let mut c = cv.fg;
            c.a *= (code - 0x2590) as f32 / 4.;
            cv.rect(cv.x0, cv.y0, cv.x1, cv.y1, c);
        }
        0x2594 => cv.block(0., 0., 1., 1. / 8.),
        0x2595 => cv.block(7. / 8., 0., 1., 1.),
        // ▖ ▗ ▘ ▙ ▚ ▛ ▜ ▝ ▞ ▟: bits are upper left, upper right, lower left, lower right.
        0x2596..=0x259f => {
            let quads = [
                0b0010, 0b0001, 0b1000, 0b1011, 0b1001, 0b1110, 0b1101, 0b0100, 0b0110, 0b0111,
            ][(code - 0x2596) as usize];
            for (bit, x, y) in [
                (0b1000, 0., 0.),
                (0b0100, 0.5, 0.),
                (0b0010, 0., 0.5),
                (0b0001, 0.5, 0.5),
            ] {
                if quads & bit != 0 {
                    cv.block(x, y, x + 0.5, y + 0.5);
                }
            }
        }
        _ => {}
    }
}

/// The arms (up, right, down, left) of the characters made of straight lines
/// meeting in the middle of the cell.
fn arms(code: u32) -> Option<[Line; 4]> {
    #[rustfmt::skip]
    const TABLE: [&str; 128] = [
        // 2500
        ".l.l", ".h.h", "l.l.", "h.h.", "", "", "", "", "", "", "", "", ".ll.", ".hl.", ".lh.", ".hh.",
        // 2510
        "..ll", "..lh", "..hl", "..hh", "ll..", "lh..", "hl..", "hh..", "l..l", "l..h", "h..l", "h..h", "lll.", "lhl.", "hll.", "llh.",
        // 2520
        "hlh.", "hhl.", "lhh.", "hhh.", "l.ll", "l.lh", "h.ll", "l.hl", "h.hl", "h.lh", "l.hh", "h.hh", ".lll", ".llh", ".hll", ".hlh",
        // 2530
        ".lhl", ".lhh", ".hhl", ".hhh", "ll.l", "ll.h", "lh.l", "lh.h", "hl.l", "hl.h", "hh.l", "hh.h", "llll", "lllh", "lhll", "lhlh",
        // 2540
        "hlll", "llhl", "hlhl", "hllh", "hhll", "llhh", "lhhl", "hhlh", "lhhh", "hlhh", "hhhl", "hhhh", "", "", "", "",
        // 2550
        ".d.d", "d.d.", ".dl.", ".ld.", ".dd.", "..ld", "..dl", "..dd", "ld..", "dl..", "dd..", "l..d", "d..l", "d..d", "ldl.", "dld.",
        // 2560
        "ddd.", "l.ld", "d.dl", "d.dd", ".dld", ".ldl", ".ddd", "ld.d", "dl.l", "dd.d", "ldld", "dldl", "dddd", "", "", "",
        // 2570
        "", "", "", "", "...l", "l...", ".l..", "..l.", "...h", "h...", ".h..", "..h.", ".h.l", "l.h.", ".l.h", "h.l.",
    ];
    let spec: [u8; 4] = TABLE
        .get(code.checked_sub(0x2500)? as usize)?
        .as_bytes()
        .try_into()
        .ok()?;
    Some(spec.map(|b| match b {
        b'l' => Line::Light,
        b'h' => Line::Heavy,
        b'd' => Line::Double,
        _ => Line::None,
    }))
}

/// A cell in logical pixels, its edges on device pixels.
struct Canvas<'a> {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    scale: f32,
    /// Light line thickness, a whole number of device pixels.
    light: f32,
    fg: Hsla,
    bg: Hsla,
    window: &'a mut Window,
}

impl Canvas<'_> {
    fn snap(&self, v: f32) -> f32 {
        (v * self.scale).round() / self.scale
    }

    /// Start of a line `width` thick, centred across the cell's width.
    fn mid_x(&self, width: f32) -> f32 {
        self.snap((self.x0 + self.x1 - width) / 2.)
    }

    fn mid_y(&self, width: f32) -> f32 {
        self.snap((self.y0 + self.y1 - width) / 2.)
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Hsla) {
        if x1 > x0 && y1 > y0 {
            self.window.paint_quad(fill(
                Bounds::from_corners(point(px(x0), px(y0)), point(px(x1), px(y1))),
                color,
            ));
        }
    }

    /// A centred line `width` thick from `a` to `b` along it.
    fn bar(&mut self, horizontal: bool, a: f32, b: f32, width: f32, color: Hsla) {
        if horizontal {
            let y = self.mid_y(width);
            self.rect(a, y, b, y + width, color);
        } else {
            let x = self.mid_x(width);
            self.rect(x, a, x + width, b, color);
        }
    }

    fn lines(&mut self, [up, right, down, left]: [Line; 4]) {
        let t = self.light;
        let w = |l: Line| match l {
            Line::None => 0.,
            Line::Light => t,
            Line::Heavy => 2. * t,
            Line::Double => 3. * t,
        };
        // Arms run into the centre far enough to cover the arms across them.
        let (vw, hw) = (w(up).max(w(down)), w(left).max(w(right)));
        let (cx, cy) = (self.mid_x(vw), self.mid_y(hw));
        let fg = self.fg;
        for (line, a, b) in [(left, self.x0, cx + vw), (right, cx, self.x1)] {
            if line != Line::None {
                self.bar(true, a, b, w(line), fg);
            }
        }
        for (line, a, b) in [(up, self.y0, cy + hw), (down, cy, self.y1)] {
            if line != Line::None {
                self.bar(false, a, b, w(line), fg);
            }
        }

        // A double line is a band three lines thick with its middle opened
        // up again. The opening runs through the centre where it meets other
        // double lines, or passes straight on when nothing crosses it;
        // otherwise it stops at the line across it.
        let through = |opposite: Line, a: Line, b: Line| {
            a == Line::Double
                || b == Line::Double
                || opposite == Line::Double && (a == Line::None || b == Line::None)
        };
        let (gx, gy, bg) = (self.mid_x(t), self.mid_y(t), self.bg);
        if left == Line::Double {
            let end = if through(right, up, down) { gx + t } else { cx };
            self.bar(true, self.x0, end, t, bg);
        }
        if right == Line::Double {
            let start = if through(left, up, down) { gx } else { cx + vw };
            self.bar(true, start, self.x1, t, bg);
        }
        if up == Line::Double {
            let end = if through(down, left, right) {
                gy + t
            } else {
                cy
            };
            self.bar(false, self.y0, end, t, bg);
        }
        if down == Line::Double {
            let start = if through(up, left, right) {
                gy
            } else {
                cy + hw
            };
            self.bar(false, start, self.y1, t, bg);
        }
    }

    fn dashes(&mut self, horizontal: bool, n: u32, width: f32) {
        let (a, b) = if horizontal {
            (self.x0, self.x1)
        } else {
            (self.y0, self.y1)
        };
        let seg = (b - a) / n as f32;
        let gap = (seg * 0.4).max(1. / self.scale);
        for i in 0..n {
            let start = self.snap(a + seg * i as f32 + gap / 2.);
            let end = self.snap(a + seg * (i + 1) as f32 - gap / 2.);
            self.bar(horizontal, start, end, width, self.fg);
        }
    }

    /// A light rounded corner joining the cell edges at `x` and `y`.
    fn rounded(&mut self, x: f32, y: f32) {
        let t = self.light;
        let (mx, my) = (self.mid_x(t) + t / 2., self.mid_y(t) + t / 2.);
        let r = (x - mx).abs().min((y - my).abs());
        let mut path = PathBuilder::stroke(px(t));
        path.move_to(point(px(mx), px(y)));
        path.line_to(point(px(mx), px(my + r * (y - my).signum())));
        path.curve_to(
            point(px(mx + r * (x - mx).signum()), px(my)),
            point(px(mx), px(my)),
        );
        path.line_to(point(px(x), px(my)));
        if let Ok(path) = path.build() {
            self.window.paint_path(path, self.fg);
        }
    }

    fn diagonal(&mut self, down: bool) {
        let (ya, yb) = if down {
            (self.y0, self.y1)
        } else {
            (self.y1, self.y0)
        };
        let mut path = PathBuilder::stroke(px(self.light));
        path.move_to(point(px(self.x0), px(ya)));
        path.line_to(point(px(self.x1), px(yb)));
        if let Ok(path) = path.build() {
            self.window.paint_path(path, self.fg);
        }
    }

    /// Fills a part of the cell given as fractions of its size.
    fn block(&mut self, fx0: f32, fy0: f32, fx1: f32, fy1: f32) {
        let x = |f: f32| self.snap(self.x0 + (self.x1 - self.x0) * f);
        let y = |f: f32| self.snap(self.y0 + (self.y1 - self.y0) * f);
        let (x0, y0, x1, y1) = (x(fx0), y(fy0), x(fx1), y(fy1));
        self.rect(x0, y0, x1, y1, self.fg);
    }
}
