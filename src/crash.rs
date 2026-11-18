//! What a panic was, written down: on macOS a panic aborts the app (it runs
//! inside the run loop, which can't unwind), and the system's crash report
//! has the stack but not the message or the line. So each panic also goes
//! to `panic.log` in the state folder, with its backtrace.
//!
//! Nothing runs until something panics: no cost while the app works.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Past this the log starts over (the previous one kept as `panic.log.old`).
const MAX_LOG: u64 = 1024 * 1024;

/// Install the hook. The default one still prints to stderr first.
pub fn install() {
    install_at(crate::channel::state_dir().join("panic.log"));
}

fn install_at(path: PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let thread = std::thread::current();
        let entry = entry(
            thread.name().unwrap_or("unnamed"),
            &message(info.payload()),
            info.location().map(|l| l.to_string()).as_deref(),
            &std::backtrace::Backtrace::force_capture().to_string(),
        );
        // Best effort: a hook that panics itself aborts with nothing written.
        let _ = append(&path, &entry);
    }));
}

/// The panic's message: what `panic!` and `.expect()` were given.
fn message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(not a string)".into())
}

fn entry(thread: &str, message: &str, location: Option<&str>, backtrace: &str) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let process = std::env::args().nth(1).unwrap_or_else(|| "app".into());
    format!(
        "=== {secs} abromux {} ({process}, pid {}) thread '{thread}' panicked at {}:\n{message}\n{backtrace}\n",
        env!("ABROMUX_VERSION"),
        std::process::id(),
        location.unwrap_or("an unknown place"),
    )
}

fn append(path: &Path, entry: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = std::fs::rename(path, old(path));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(entry.as_bytes())
}

fn old(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".old");
    s.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_is_written_with_its_message_and_place() {
        let dir = std::env::temp_dir().join(format!("abromux-crash-{}", std::process::id()));
        let path = dir.join("panic.log");
        let _ = std::fs::remove_dir_all(&dir);
        let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("spawn shell: boom"));
        let e = entry("main", &message(&*payload), Some("src/pty.rs:317:5"), "bt");
        append(&path, &e).unwrap();
        append(&path, &e).unwrap();
        let log = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(log.matches("=== ").count(), 2);
        assert!(log.contains("thread 'main' panicked at src/pty.rs:317:5:\nspawn shell: boom\nbt"));
    }

    #[test]
    fn the_hook_writes_a_real_panic() {
        let dir = std::env::temp_dir().join(format!("abromux-crash-hook-{}", std::process::id()));
        let path = dir.join("panic.log");
        let _ = std::fs::remove_dir_all(&dir);
        install_at(path.clone());
        let caught = std::thread::Builder::new()
            .name("crash-test".into())
            .spawn(|| panic!("hook test {}", 42))
            .unwrap()
            .join();
        let _ = std::panic::take_hook(); // back to the default for other tests
        let log = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(caught.is_err());
        assert!(
            log.contains("thread 'crash-test' panicked at src/crash.rs:"),
            "{log}"
        );
        assert!(log.contains("hook test 42"), "{log}");
    }

    #[test]
    fn a_full_log_starts_over_keeping_the_last() {
        let dir = std::env::temp_dir().join(format!("abromux-crash-full-{}", std::process::id()));
        let path = dir.join("panic.log");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, vec![b'x'; MAX_LOG as usize + 1]).unwrap();
        append(&path, "new\n").unwrap();
        let now = std::fs::read_to_string(&path).unwrap();
        let kept = std::fs::metadata(old(&path)).map(|m| m.len());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(now, "new\n");
        assert_eq!(kept.ok(), Some(MAX_LOG + 1));
    }
}
