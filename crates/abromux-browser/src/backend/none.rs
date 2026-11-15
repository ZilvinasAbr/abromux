//! No built-in browser (Linux, for now): the pane says so, and everything
//! that would ask the page answers that there's none. Phase 3 of
//! docs/linux-plan.md hands browser tabs to other engines instead.

use super::{Answer, PageBackend, PageEvent, PageOptions, Pixels};
use futures::channel::{mpsc, oneshot};
use gpui::{AnyElement, App, IntoElement, ParentElement, Styled, div};

const NONE: &str = "no built-in browser on this system";

pub struct NoPage;

impl NoPage {
    pub fn new(_options: PageOptions, _events: mpsc::UnboundedSender<PageEvent>) -> Self {
        NoPage
    }
}

fn answer<T>(value: T) -> oneshot::Receiver<T> {
    let (tx, rx) = oneshot::channel();
    let _ = tx.send(value);
    rx
}

fn none() -> oneshot::Receiver<Answer> {
    answer(Err(NONE.into()))
}

impl PageBackend for NoPage {
    fn element(&self) -> AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child("No built-in browser on Linux yet")
            .into_any_element()
    }
    fn set_shown(&self, _shown: bool, _cx: &mut App) {}
    fn load_url(&self, _url: &str, _cx: &mut App) {}
    fn reload(&self, _cx: &App) {}
    fn stop(&self) {}
    fn is_loading(&self) -> bool {
        false
    }
    fn set_zoom(&self, _zoom: f64) {}
    fn run(&self, _js: &str, _cx: &App) {}
    fn eval(&self, _js: &str, _cx: &App) -> oneshot::Receiver<Answer> {
        none()
    }
    fn eval_in_frame(&self, _token: &str, _js: &str) -> oneshot::Receiver<Answer> {
        none()
    }
    fn frame_known(&self, _token: &str) -> bool {
        false
    }
    fn history_state(&self) -> Option<String> {
        None
    }
    fn restore_history(&self, _state: &str) -> bool {
        false
    }
    fn focus(&self, _cx: &App) {}
    fn unfocus(&self, _cx: &App) {}
    fn has_focus(&self) -> bool {
        false
    }
    fn bitmap(&self) -> oneshot::Receiver<Option<Pixels>> {
        answer(None)
    }
    fn png(&self) -> oneshot::Receiver<Option<Vec<u8>>> {
        answer(None)
    }
    fn cookies(&self) -> oneshot::Receiver<Answer> {
        none()
    }
    fn set_cookies(&self, _json: &str) -> oneshot::Receiver<Answer> {
        none()
    }
    fn delete_cookies(&self, _json: &str) -> oneshot::Receiver<Answer> {
        none()
    }
    fn hover(&self, _url: &str) {}
    fn show_inspector(&self, _console: bool) {}
}
