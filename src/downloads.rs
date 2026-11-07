//! Downloads from browser panes, for the toolbar's downloads button and
//! `abromux browser downloads` / `wait-download`. WebKit saves them (wry
//! picks a free name in ~/Downloads); this keeps the list for the session,
//! and follows each running one's progress.

use crate::browser_native;
use gpui::{App, AppContext as _, Context, Entity, Timer};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

const POLL: Duration = Duration::from_millis(250);
/// Finished downloads kept in the list.
const KEEP: usize = 50;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    Running,
    Done,
    Failed,
    Canceled,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Done => "done",
            State::Failed => "failed",
            State::Canceled => "canceled",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Download {
    pub id: u64,
    pub url: String,
    pub path: PathBuf,
    pub state: State,
    pub written: u64,
    pub total: Option<u64>,
    pub started: SystemTime,
    pub finished: Option<SystemTime>,
}

impl Download {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.url.clone())
    }

    /// 0…1 while running with a known size.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.filter(|&t| t > 0)?;
        Some((self.written as f64 / total as f64).clamp(0., 1.) as f32)
    }

    pub fn to_json(&self) -> Value {
        let secs = |t: SystemTime| {
            t.duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        };
        json!({
            "id": self.id,
            "url": self.url,
            "path": self.path,
            "name": self.name(),
            "state": self.state.name(),
            "bytes": self.written,
            "total": self.total,
            "started": secs(self.started),
            "finished": self.finished.map(secs),
        })
    }
}

pub struct Downloads {
    pub list: Vec<Download>,
    next: u64,
    polling: bool,
    /// Bumped when a download starts, for the button's bounce.
    pub starts: usize,
}

struct Global(Entity<Downloads>);
impl gpui::Global for Global {}

pub fn entity(cx: &mut App) -> Entity<Downloads> {
    if let Some(g) = cx.try_global::<Global>() {
        return g.0.clone();
    }
    let e = cx.new(|_| Downloads {
        list: vec![],
        next: 1,
        polling: false,
        starts: 0,
    });
    cx.set_global(Global(e.clone()));
    e
}

impl Downloads {
    pub fn get(&self, id: u64) -> Option<&Download> {
        self.list.iter().find(|d| d.id == id)
    }

    pub fn last_id(&self) -> u64 {
        self.next - 1
    }

    pub fn running(&self) -> bool {
        self.list.iter().any(|d| d.state == State::Running)
    }

    pub fn started(&mut self, url: String, path: PathBuf, cx: &mut Context<Self>) -> u64 {
        let id = self.next;
        self.next += 1;
        self.list.push(Download {
            id,
            url,
            path,
            state: State::Running,
            written: 0,
            total: None,
            started: SystemTime::now(),
            finished: None,
        });
        self.starts += 1;
        let done = self
            .list
            .iter()
            .filter(|d| d.state != State::Running)
            .count();
        if done > KEEP
            && let Some(i) = self.list.iter().position(|d| d.state != State::Running)
        {
            self.list.remove(i);
        }
        self.poll(cx);
        cx.notify();
        id
    }

    /// wry's completion: the newest running download from `url`.
    pub fn finished(&mut self, url: &str, ok: bool, cx: &mut Context<Self>) {
        browser_native::download_forget(url);
        let Some(d) = self
            .list
            .iter_mut()
            .rev()
            .find(|d| d.url == url && d.state == State::Running)
        else {
            return;
        };
        d.finished = Some(SystemTime::now());
        if ok {
            d.state = State::Done;
            if let Ok(m) = std::fs::metadata(&d.path) {
                d.written = m.len();
                d.total = Some(m.len());
            }
        } else {
            d.state = State::Failed;
        }
        cx.notify();
    }

    /// `wait-download --path` moved it.
    pub fn moved(&mut self, id: u64, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(d) = self.list.iter_mut().find(|d| d.id == id) {
            d.path = path;
            cx.notify();
        }
    }

    pub fn cancel(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let Some(d) = self.list.iter_mut().find(|d| d.id == id) else {
            return false;
        };
        if d.state != State::Running {
            return false;
        }
        // Marked first, so wry's failure report that follows keeps it.
        d.state = State::Canceled;
        d.finished = Some(SystemTime::now());
        let url = d.url.clone();
        browser_native::download_cancel(&url);
        browser_native::download_forget(&url);
        let _ = std::fs::remove_file(&d.path);
        cx.notify();
        true
    }

    /// Drop finished downloads from the list (the files stay).
    pub fn clear(&mut self, cx: &mut Context<Self>) -> usize {
        let before = self.list.len();
        self.list.retain(|d| d.state == State::Running);
        cx.notify();
        before - self.list.len()
    }

    /// Follow running downloads' progress until none are left.
    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(POLL).await;
                let more = this.update(cx, |this, cx| {
                    let mut changed = false;
                    for d in this.list.iter_mut().filter(|d| d.state == State::Running) {
                        if let Some((written, total)) = browser_native::download_progress(&d.url)
                            && (written, total) != (d.written, d.total)
                        {
                            (d.written, d.total) = (written, total);
                            changed = true;
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                    this.polling = this.running();
                    this.polling
                });
                if !more.unwrap_or(false) {
                    return;
                }
            }
        })
        .detach();
    }
}

/// "1.2 MB", for the list.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64 / 1000.;
    let mut unit = 0;
    while v >= 1000. && unit < UNITS.len() - 1 {
        v /= 1000.;
        unit += 1;
    }
    if v < 10. {
        format!("{v:.1} {}", UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::size;

    #[test]
    fn sizes_read_like_finders() {
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(1_500), "1.5 KB");
        assert_eq!(size(23_400_000), "23 MB");
        assert_eq!(size(4_200_000_000), "4.2 GB");
    }
}
