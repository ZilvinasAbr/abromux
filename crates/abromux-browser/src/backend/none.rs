//! No built-in browser (Linux): a browser tab is its address bar over a
//! note saying where its page opens. Going to an address (typing one, Back,
//! Reload) hands it to the default browser; making the tab doesn't, so a
//! restored session or a layout opens nothing by itself. Everything that
//! would ask the page answers that it needs the built-in browser.
//! (docs/linux-plan.md, Phase 3.)

use super::{Answer, PageBackend, PageEvent, PageOptions, Pixels};
use futures::channel::{mpsc, oneshot};
use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, div, px,
};
use std::{cell::RefCell, rc::Rc};

const NONE: &str = "this needs the built-in browser, which only macOS has \
    (browser.engine \"terminalBrowser\" runs terminal-browser in a tab)";

pub struct NoPage {
    url: Rc<RefCell<String>>,
    events: mpsc::UnboundedSender<PageEvent>,
}

impl NoPage {
    pub fn new(options: PageOptions, events: mpsc::UnboundedSender<PageEvent>) -> Self {
        NoPage {
            url: Rc::new(RefCell::new(options.url)),
            events,
        }
    }
}

/// Hand `url` to the default browser; blank pages stay here.
fn open(url: &str) {
    if url.is_empty() || url == "about:blank" {
        return;
    }
    if let Err(e) = abromux_platform::opener::open(url) {
        eprintln!("abromux: opening {url}: {e}");
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
        let url = self.url.borrow().clone();
        let note = div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2();
        if url.is_empty() || url == "about:blank" {
            return note
                .child("Type an address above: it opens in your default browser.")
                .into_any_element();
        }
        let link = self.url.clone();
        note.child(
            div()
                .w_full()
                .max_w(px(560.))
                .text_center()
                .truncate()
                .child(url),
        )
        .child("opens in your default browser.")
        .child(
            div()
                .id("open-in-browser")
                .px_3()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(gpui::rgba(0x8080807f))
                .cursor_pointer()
                .child("Open in browser")
                .on_click(move |_, _, _| open(&link.borrow())),
        )
        .into_any_element()
    }
    fn set_shown(&self, _shown: bool, _cx: &mut App) {}
    fn load_url(&self, url: &str, _cx: &mut App) {
        *self.url.borrow_mut() = url.to_string();
        let _ = self.events.unbounded_send(PageEvent::Url {
            url: url.to_string(),
            finished: true,
        });
        open(url);
    }
    fn reload(&self, _cx: &App) {
        open(&self.url.borrow());
    }
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
    /// Taken as done, so a restored tab isn't sent to its page (which would
    /// open it in the default browser at every launch).
    fn restore_history(&self, _state: &str) -> bool {
        true
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
