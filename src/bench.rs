//! `abromux bench-vt <file>...`: feed files through libghostty-vt with no window,
//! the way a terminal pane does (64 KiB reads, a frame snapshot at most every
//! 16 ms), and print `<file> <bytes> <seconds>` per pass. Used by
//! scripts/perf.py; independent of window state, App Nap and the shell.

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
