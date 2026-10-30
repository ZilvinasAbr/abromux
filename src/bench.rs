//! `abromux bench-vt <file>...`: feed files through libghostty-vt with no window,
//! the way a terminal pane does (64 KiB reads, a frame snapshot at most every
//! 16 ms), and print `<file> <bytes> <seconds>` per pass. Used by
//! scripts/perf.py; independent of window state, App Nap and the shell. Also
//! `abromux bench-kitty`, the same for kitty graphics frames.

use crate::ghostty::Terminal;
use std::time::{Duration, Instant};

const CHUNK: usize = 64 * 1024;
const FRAME: Duration = Duration::from_millis(16);

pub fn vt(args: &[String]) -> i32 {
    let passes: usize = std::env::var("ABROMUX_BENCH_PASSES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    for path in args {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("abromux: {path}: {e}");
                return 1;
            }
        };
        for _ in 0..passes {
            // Same grid as bench.sh's 1000x700 window at 13 pt.
            let mut term = match Terminal::new(96, 41) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("abromux: {e}");
                    return 1;
                }
            };
            let mut frame = Default::default();
            let start = Instant::now();
            let mut last_frame = start;
            for chunk in data.chunks(CHUNK) {
                term.write(chunk);
                term.take_effects();
                if last_frame.elapsed() >= FRAME {
                    term.update_frame(&mut frame);
                    std::hint::black_box(&frame);
                    last_frame = Instant::now();
                }
            }
            term.update_frame(&mut frame);
            std::hint::black_box(&frame);
            println!("{path} {} {:.6}", data.len(), start.elapsed().as_secs_f64());
        }
    }
    0
}

/// `abromux bench-kitty [frames]`: send full-pane kitty images the way
/// terminal-browser does, one new frame replacing the last, and time each
/// stage a pane runs on the main thread: parsing the escape sequences, the
/// frame snapshot, and converting the pixels to the BGRA a texture needs.
/// Prints `<parse ms> <snapshot ms> <convert ms>` per frame, averaged, per pass.
pub fn kitty(args: &[String]) -> i32 {
    let passes: usize = std::env::var("ABROMUX_BENCH_PASSES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let frames: usize = args.first().and_then(|v| v.parse().ok()).unwrap_or(20);
    // A 1000x700 pt pane on a 2x display, as terminal-browser fills it.
    let (w, h) = (1920, 1200);
    let images: Vec<Vec<u8>> = (0..2u8).map(|i| kitty_image(w, h, i)).collect();
    for _ in 0..passes {
        let mut term = match Terminal::new(96, 41) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("abromux: {e}");
                return 1;
            }
        };
        term.resize(96, 41, 20, 36);
        let mut frame = Default::default();
        let (mut parse, mut snapshot, mut convert) =
            (Duration::ZERO, Duration::ZERO, Duration::ZERO);
        for n in 0..frames {
            let t = Instant::now();
            for chunk in images[n % 2].chunks(CHUNK) {
                term.write(chunk);
                term.take_effects();
            }
            parse += t.elapsed();
            let t = Instant::now();
            term.update_frame(&mut frame);
            snapshot += t.elapsed();
            let t = Instant::now();
            for img in &frame.images {
                std::hint::black_box(term.image_pixels(img.id, img.src));
            }
            convert += t.elapsed();
            if frame.images.is_empty() {
                eprintln!("abromux: the image wasn't placed");
                return 1;
            }
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000. / frames as f64;
        println!("{:.4} {:.4} {:.4}", ms(parse), ms(snapshot), ms(convert));
    }
    0
}

/// One frame: home the cursor, then transmit and place image 1 as RGBA in
/// 4096-byte base64 chunks, like kitty's own clients. `seed` varies the pixels.
fn kitty_image(w: usize, h: usize, seed: u8) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            rgba.extend([
                (x / 8) as u8 ^ seed,
                (y / 5) as u8,
                seed.wrapping_mul(90),
                255,
            ]);
        }
    }
    let b64 = base64(&rgba);
    let mut out = b"\x1b[H".to_vec();
    let chunks: Vec<&[u8]> = b64.chunks(4096).collect();
    for (i, c) in chunks.iter().enumerate() {
        let more = (i + 1 < chunks.len()) as u8;
        if i == 0 {
            out.extend(format!("\x1b_Ga=T,f=32,s={w},v={h},i=1,C=1,q=2,m={more};").bytes());
        } else {
            out.extend(format!("\x1b_Gm={more};").bytes());
        }
        out.extend_from_slice(c);
        out.extend(b"\x1b\\");
    }
    out
}

fn base64(data: &[u8]) -> Vec<u8> {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() {
                A[(n >> (18 - 6 * i) & 63) as usize]
            } else {
                b'='
            });
        }
    }
    out
}
