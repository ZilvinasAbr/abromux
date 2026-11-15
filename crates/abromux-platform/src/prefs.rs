//! Small app preferences that aren't settings: the "Don't ask again" boxes
//! and the like. A JSON object in `prefs.json` in the support folder, the
//! same file on every OS. (macOS builds kept these in NSUserDefaults before;
//! [`import_once`] brings them over.)

use serde_json::{Map, Value};
use std::{path::PathBuf, sync::Mutex};

/// The file's contents, read on first use.
static PREFS: Mutex<Option<Map<String, Value>>> = Mutex::new(None);

fn path() -> PathBuf {
    crate::dirs::support_dir().join("prefs.json")
}

fn with<T>(f: impl FnOnce(&mut Map<String, Value>) -> T) -> T {
    let mut prefs = PREFS.lock().unwrap();
    let map = prefs.get_or_insert_with(|| read(&path()).unwrap_or_default());
    f(map)
}

fn read(path: &std::path::Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write(map: &Map<String, Value>) -> std::io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(map).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, text + "\n")?;
    std::fs::rename(&tmp, &path)
}

/// The boolean at `key`; false when it isn't set.
pub fn bool(key: &str) -> bool {
    with(|m| m.get(key).and_then(Value::as_bool).unwrap_or(false))
}

/// Set `key` and save the file.
pub fn set_bool(key: &str, value: bool) {
    with(|m| {
        m.insert(key.into(), Value::Bool(value));
        let _ = write(m);
    });
}

/// The first time there's no prefs file, copy `keys` over from where this
/// OS kept them before (NSUserDefaults on macOS), and write the file so it
/// happens only once. Keys that were never set stay unset. Only the app
/// itself does it: a bare binary shares the release's support folder but
/// not its defaults, and would mark the import done with nothing in it.
pub fn import_once(keys: &[&str]) {
    if !crate::os::defaults::bundled() || path().exists() {
        return;
    }
    with(|m| {
        for key in keys {
            if crate::os::defaults::bool(key) {
                m.insert((*key).into(), Value::Bool(true));
            }
        }
        let _ = write(m);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_objects() {
        let dir = std::env::temp_dir().join(format!("abromux-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("prefs.json");
        std::fs::write(&file, r#"{"A": true, "B": false}"#).unwrap();
        let m = read(&file).unwrap();
        assert_eq!(m.get("A"), Some(&Value::Bool(true)));
        std::fs::write(&file, "[1]").unwrap();
        assert!(read(&file).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
