#![allow(unexpected_cfgs)]
//! Dev helpers: `ABROMUX_DEMO=1` builds a sample layout and `ABROMUX_SNAPSHOT=out.png`
//! asks the driving shell to
//! save a PNG of our window (see scripts/drive.sh).

use gpui::Window;
use objc::{msg_send, runtime::Object, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub fn snapshot(window: &Window, path: &str) -> anyhow::Result<()> {
    let RawWindowHandle::AppKit(h) = HasWindowHandle::window_handle(window)?.as_raw() else {
        anyhow::bail!("not an AppKit window")
    };
    let number: isize = unsafe {
        let view = h.ns_view.as_ptr() as *mut Object;
        let win: *mut Object = msg_send![view, window];
        msg_send![win, windowNumber]
    };
    // Capture is done by Snapper.app (it holds the Screen Recording
    // permission): the driving shell watches stdout for this line.
    use std::io::Write;
    println!("SNAP {number} {path}");
    std::io::stdout().flush()?;
    Ok(())
}

/// Drive the app for testing. `ABROMUX_SCRIPT` is `;`-separated steps:
/// `wait:MS`, `key:cmd-n` (gpui keystroke syntax), `type:TEXT`, `snap:PATH`, `quit`,
/// `mouse:down|up|move,X,Y[,left|right|middle][,shift]` and `scroll:X,Y,DY`
/// (window pixels; positive DY scrolls up), and `mods:cmd|ctrl|none` for a
/// modifier-only change.
/// Mouse steps go to `on_mouse` (gpui's own dispatch_event isn't callable
/// from outside the crate).
pub fn run_script<V: 'static>(
    script: String,
    on_mouse: fn(&mut V, gpui::PlatformInput, &mut Window, &mut gpui::Context<V>),
    window: &mut Window,
    cx: &mut gpui::Context<V>,
) {
    use gpui::{Keystroke, Timer};
    use std::time::Duration;
    cx.spawn_in(window, async move |this, cx| {
        Timer::after(Duration::from_millis(2500)).await;
        for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (cmd, arg) = step.split_once(':').unwrap_or((step, ""));
            if cmd == "wait" {
                Timer::after(Duration::from_millis(arg.parse().unwrap_or(500))).await;
                continue;
            }
            if cmd == "mouse" || cmd == "scroll" || cmd == "mods" {
                match mouse_event(cmd, arg) {
                    Some(ev) => {
                        let _ = this.update_in(cx, |v, window, cx| on_mouse(v, ev, window, cx));
                    }
                    None => eprintln!("bad {cmd} step {arg}"),
                }
                continue;
            }
            let (cmd, arg) = (cmd.to_string(), arg.to_string());
            let _ = cx.update(move |window, cx| match cmd.as_str() {
                "key" => match Keystroke::parse(&arg) {
                    Ok(ks) => {
                        window.dispatch_keystroke(ks, cx);
                    }
                    Err(e) => eprintln!("bad key {arg}: {e:?}"),
                },
                "type" => {
                    for ch in arg.chars() {
                        let key = if ch == ' ' { "space".to_string() } else { ch.to_lowercase().to_string() };
                        let mut ks = Keystroke::parse(&key).unwrap();
                        ks.modifiers.shift = ch.is_uppercase();
                        ks.key_char = Some(ch.to_string());
                        window.dispatch_keystroke(ks, cx);
                    }
                }
                "snap" => {
                    if let Err(e) = snapshot(window, &arg) {
                        eprintln!("snapshot failed: {e}");
                    }
                }
                "quit" => cx.quit(),
                other => eprintln!("unknown step {other}"),
            });
        }
    })
    .detach();
}

fn mouse_event(cmd: &str, arg: &str) -> Option<gpui::PlatformInput> {
    use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, ScrollDelta, ScrollWheelEvent, point, px};
    let parts: Vec<&str> = arg.split(',').map(str::trim).collect();
    let num = |i: usize| parts.get(i).and_then(|v| v.parse::<f32>().ok());
    let modifiers = Modifiers { shift: parts.contains(&"shift"), ..Default::default() };
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
