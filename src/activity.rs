#![allow(unexpected_cfgs)]
//! Keeps macOS from App Napping abromux while a terminal is printing.
//! Hidden or covered for a while, the app is napped and its main thread
//! (where output is drawn) slows several times over. A shell printing is
//! work the user asked for, so hold an NSProcessInfo activity while output
//! flows and let it go after a quiet spell.

use objc::{class, msg_send, runtime::Object, sel, sel_impl};
use std::{
    ffi::CString,
    sync::Mutex,
    time::{Duration, Instant},
};

/// NSActivityUserInitiatedAllowingIdleSystemSleep: no App Nap, but the Mac
/// may still sleep when idle.
const OPTIONS: u64 = 0x00FF_FFFF & !(1 << 20);
/// How long output must stop before the activity ends.
const QUIET: Duration = Duration::from_secs(2);

struct State {
    /// The NSActivity token, retained; 0 when none is held.
    token: usize,
    last: Instant,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Note that a terminal produced output. Cheap enough to call per read;
/// safe from any thread.
pub fn output() {
    let mut state = STATE.lock().unwrap();
    if let Some(s) = state.as_mut() {
        s.last = Instant::now();
        return;
    }
    *state = Some(State {
        token: begin(),
        last: Instant::now(),
    });
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(QUIET);
            let mut state = STATE.lock().unwrap();
            if state.as_ref().is_some_and(|s| s.last.elapsed() >= QUIET) {
                end(state.take().unwrap().token);
                return;
            }
        }
    });
}

fn begin() -> usize {
    let reason = CString::new("Terminal output").unwrap();
    unsafe {
        let reason: *mut Object = msg_send![class!(NSString), stringWithUTF8String: reason.as_ptr()];
        let info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        let token: *mut Object = msg_send![info, beginActivityWithOptions: OPTIONS reason: reason];
        let token: *mut Object = msg_send![token, retain];
        token as usize
    }
}

fn end(token: usize) {
    unsafe {
        let token = token as *mut Object;
        let info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        let _: () = msg_send![info, endActivity: token];
        let _: () = msg_send![token, release];
    }
}
