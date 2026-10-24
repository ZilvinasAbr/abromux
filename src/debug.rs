#![allow(unexpected_cfgs)]
//! Dev helpers: `ABROMUX_DEMO=1` builds a sample layout and `ABROMUX_SNAPSHOT=out.png`
//! asks the driving shell to
//! save a PNG of our window (see scripts/drive.sh). Also the frame timing and
//! synthetic input behind the `debug.*` RPCs that scripts/perf.py uses.

use anyhow::{Context as _, anyhow, bail};
use futures::channel::oneshot;
use gpui::{App, Context, Entity, Keystroke, PlatformInput, Timer, Window};
use objc::{msg_send, runtime::Object, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

pub fn snapshot(window: &Window, path: &str) -> anyhow::Result<()> {
    let RawWindowHandle::AppKit(h) = HasWindowHandle::window_handle(window)?.as_raw() else {
        anyhow::bail!("not an AppKit window")
    };
    let number: isize = unsafe {
        let view = h.ns_view.as_ptr() as *mut Object;
        let win: *mut Object = msg_send![view, window];
        msg_send![win, windowNumber]
    };
    // Capture is done by scripts/drive.sh with screencapture (under the
    // terminal's Screen Recording permission): it watches stdout for this line.
    use std::io::Write;
    println!("SNAP {number} {path}");
    std::io::stdout().flush()?;
    Ok(())
}

/// Mouse steps go to this (gpui's own dispatch_event isn't callable from
/// outside the crate).
pub type OnMouse<V> = fn(&mut V, PlatformInput, &mut Window, &mut Context<V>);

/// Drive the app for testing. `ABROMUX_SCRIPT` is `;`-separated steps:
/// `wait:MS`, `key:cmd-n` (gpui keystroke syntax), `type:TEXT`, `snap:PATH`, `quit`,
/// `mouse:down|up|move,X,Y[,left|right|middle][,shift][,cmd]` and `scroll:X,Y,DY`
/// (window pixels; positive DY scrolls up), and `mods:cmd|ctrl|none` for a
/// modifier-only change.
pub fn run_script<V: 'static>(script: String, on_mouse: OnMouse<V>, window: &mut Window, cx: &mut Context<V>) {
    cx.spawn_in(window, async move |this, cx| {
        Timer::after(Duration::from_millis(2500)).await;
        for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            if let ("wait", arg) = step.split_once(':').unwrap_or((step, "")) {
                Timer::after(Duration::from_millis(arg.parse().unwrap_or(500))).await;
                continue;
            }
            let Some(view) = this.upgrade() else { return };
            let _ = cx.update(|window, cx| {
                if let Err(e) = run_step(&view, step, on_mouse, window, cx) {
                    eprintln!("{e:#}");
                }
            });
        }
    })
    .detach();
}

/// One script step other than `wait`. Also behind the `debug.input` RPC.
pub fn run_step<V: 'static>(view: &Entity<V>, step: &str, on_mouse: OnMouse<V>, window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
    let (cmd, arg) = step.split_once(':').unwrap_or((step, ""));
    match cmd {
        "mouse" | "scroll" | "mods" => {
            let ev = mouse_event(cmd, arg).with_context(|| format!("bad {cmd} step {arg}"))?;
            view.update(cx, |v, cx| on_mouse(v, ev, window, cx));
        }
        "key" => {
            let ks = Keystroke::parse(arg).map_err(|e| anyhow!("bad key {arg}: {e:?}"))?;
            window.dispatch_keystroke(ks, cx);
        }
        "type" => {
            for ch in arg.chars() {
                let key = if ch == ' ' { "space".to_string() } else { ch.to_lowercase().to_string() };
                let mut ks = Keystroke::parse(&key).map_err(|e| anyhow!("can't type {ch:?}: {e:?}"))?;
                ks.modifiers.shift = ch.is_uppercase();
                ks.key_char = Some(ch.to_string());
                window.dispatch_keystroke(ks, cx);
            }
        }
        "snap" => snapshot(window, arg).context("snapshot failed")?,
        "quit" => cx.quit(),
        other => bail!("unknown step {other}"),
    }
    Ok(())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}

/// Resolves once the next frame has been drawn and presented, with how long
/// the frame took to start (`wait_ms`) and to draw (`draw_ms`). Frame
/// callbacks run just before the frame's draw, so a task spawned from one
/// runs right after it. Gives up after a second: a hidden window gets no
/// frames.
pub fn after_next_frame(window: &Window, cx: &mut App) -> oneshot::Receiver<anyhow::Result<Value>> {
    let (tx, rx) = oneshot::channel();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let requested = Instant::now();
    let on_frame = tx.clone();
    window.on_next_frame(move |_, cx| {
        let start = Instant::now();
        cx.spawn(async move |_| {
            if let Some(tx) = on_frame.borrow_mut().take() {
                let _ = tx.send(Ok(json!({ "wait_ms": ms(start - requested), "draw_ms": ms(start.elapsed()) })));
            }
        })
        .detach();
    });
    cx.spawn(async move |_| {
        Timer::after(Duration::from_secs(1)).await;
        if let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.send(Err(anyhow!("no frame within 1 s (is the window visible?)")));
        }
    })
    .detach();
    rx
}

/// Frame timings while recording (`debug.frames`): the gap between frame
/// starts, one display-link tick per vsync unless the main thread overran,
/// and each frame's draw, from its start to just after it was presented.
#[derive(Default)]
pub struct FrameLog {
    generation: usize,
    recording: bool,
    last: Option<Instant>,
    intervals: Vec<f64>,
    draws: Vec<f64>,
}

impl FrameLog {
    pub fn start(log: &Rc<RefCell<Self>>, window: &Window) {
        let generation = {
            let mut l = log.borrow_mut();
            *l = FrameLog { generation: l.generation + 1, recording: true, ..Default::default() };
            l.generation
        };
        Self::next(log.clone(), generation, window);
    }

    pub fn stop(log: &Rc<RefCell<Self>>) -> Value {
        let mut l = log.borrow_mut();
        l.recording = false;
        json!({ "interval_ms": std::mem::take(&mut l.intervals), "draw_ms": std::mem::take(&mut l.draws) })
    }

    fn next(log: Rc<RefCell<Self>>, generation: usize, window: &Window) {
        window.on_next_frame(move |window, cx| {
            let start = Instant::now();
            {
                let mut l = log.borrow_mut();
                if !l.recording || l.generation != generation {
                    return;
                }
                if let Some(last) = l.last.replace(start) {
                    l.intervals.push(ms(start - last));
                }
            }
            let draws = log.clone();
            cx.spawn(async move |_| draws.borrow_mut().draws.push(ms(start.elapsed()))).detach();
            Self::next(log, generation, window);
        });
    }
}

fn mouse_event(cmd: &str, arg: &str) -> Option<PlatformInput> {
    use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollDelta, ScrollWheelEvent, point, px};
    let parts: Vec<&str> = arg.split(',').map(str::trim).collect();
    let num = |i: usize| parts.get(i).and_then(|v| v.parse::<f32>().ok());
    let modifiers = Modifiers { shift: parts.contains(&"shift"), platform: parts.contains(&"cmd"), ..Default::default() };
    if cmd == "mods" {
        let modifiers = Modifiers { platform: parts.contains(&"cmd"), control: parts.contains(&"ctrl"), ..Default::default() };
        return Some(PlatformInput::ModifiersChanged(gpui::ModifiersChangedEvent { modifiers, ..Default::default() }));
    }
    if cmd == "scroll" {
        let position = point(px(num(0)?), px(num(1)?));
        let delta = ScrollDelta::Pixels(point(px(0.), px(num(2)?)));
        return Some(PlatformInput::ScrollWheel(ScrollWheelEvent { position, delta, modifiers, ..Default::default() }));
    }
    let position = point(px(num(1)?), px(num(2)?));
    let button = match parts.get(3).copied() {
        Some("right") => MouseButton::Right,
        Some("middle") => MouseButton::Middle,
        _ => MouseButton::Left,
    };
    Some(match parts[0] {
        "down" => PlatformInput::MouseDown(MouseDownEvent { button, position, modifiers, click_count: 1, first_mouse: false }),
        "up" => PlatformInput::MouseUp(MouseUpEvent { button, position, modifiers, click_count: 1 }),
        "move" => PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: parts.get(3).map(|_| button), modifiers }),
        _ => return None,
    })
}
