//! The sidebar's "Work items", as GitLab 19 has it: the issues linked to
//! the merge request with how (Closes, Related, Mentioned), each opening
//! in a browser tab and, unless the description links it, unlinked with
//! ×; + opens "Link work item" (a relationship, `#12` or a link, Add, or
//! Create new item). Threads get "Create issue to resolve thread".

#[allow(unused_imports)]
use crate::style::px;
use crate::{review::Review, style::*, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::Input;

/// The issue iid in `#12`, `12` or an issue link.
pub fn issue_iid(text: &str) -> Option<u64> {
    let t = text.trim();
    if let Some(n) = t.strip_prefix('#').or(Some(t)).and_then(|n| n.parse().ok()) {
        return Some(n);
    }
    let after = t
        .split("/-/issues/")
        .nth(1)
        .or_else(|| t.split("/-/work_items/").nth(1))?;
    after.split(['/', '?', '#']).next()?.parse().ok()
}

fn type_word(t: &str) -> &'static str {
    match t {
        "CLOSES" => "Closes",
        "MENTIONED" => "Mentions",
        _ => "Related",
    }
}

impl GitLabView {
    fn full_path(&self) -> Option<String> {
        self.review.ready().map(|r| {
            r.mr.references
                .full
                .split('!')
                .next()
                .unwrap_or_default()
                .to_string()
        })
    }

    /// Link the item typed as the relationship picked.
    pub(crate) fn link_work_item(
        &mut self,
        create: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(full) = self.full_path() else { return };
        let text = self.more.wi_input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let issue = issue_iid(&text);
        if !create && issue.is_none() {
            self.write_error =
                Some("Type an issue's #number or link (or Create new item with a title)".into());
            cx.notify();
            return;
        }
        let kind = self.more.wi_type;
        self.more
            .wi_input
            .update(cx, |b, cx| b.set_value("", window, cx));
        self.more.wi_open = false;
        self.write(
            crate::edit::Write::Sidebar,
            None,
            move |c, p, iid| {
                let issue = match (create, issue) {
                    (false, Some(n)) => n,
                    _ => c.create_issue(p, &text)?,
                };
                c.link_work_item(&full, p, iid, issue, kind)
            },
            |_, _, _| {},
            window,
            cx,
        );
    }

    pub(crate) fn unlink_work_item(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(full) = self.full_path() else { return };
        self.write(
            crate::edit::Write::Sidebar,
            None,
            move |c, _, iid| c.unlink_work_item(&full, iid, &id),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// "Create issue to resolve thread": the issue is made from the thread
    /// (GitLab resolves it) and opens beside.
    pub(crate) fn issue_for_thread(
        &mut self,
        thread: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(c), Some(p), Some(iid)) = (self.client.clone(), self.loc.project, self.loc.iid)
        else {
            return;
        };
        let title = self
            .review
            .ready()
            .map(|r| r.mr.title.clone())
            .unwrap_or_default();
        cx.spawn_in(window, async move |this, cx| {
            let got = cx
                .background_spawn(async move { c.issue_for_thread(p, iid, &thread, &title) })
                .await;
            let _ = this.update_in(cx, |v, window, cx| {
                match got {
                    Ok(url) => {
                        if !url.is_empty() {
                            abromux_plugin::open_url(&url, window, cx);
                        }
                        v.refresh(cx);
                    }
                    Err(e) => v.write_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn work_items_block(&self, r: &Review, cx: &mut Context<Self>) -> Div {
        let editable = self.me.is_some() && !self.is_local();
        let plus = editable.then(|| {
            crate::edit::icon_button("gl-wi-add", icon("plus", 15., c(TEXT)), "Add a work item")
                .size(px(24.))
                .on_click(cx.listener(|v, _, window, cx| {
                    v.more.wi_open = !v.more.wi_open;
                    if v.more.wi_open {
                        v.more.wi_input.update(cx, |b, cx| b.focus(window, cx));
                    }
                    cx.notify();
                }))
                .into_any_element()
        });
        let row = |id: String,
                   label: Option<&'static str>,
                   iid: u64,
                   title: String,
                   state: String,
                   url: String| {
            div()
                .id(SharedString::from(id))
                .flex()
                .gap_1()
                .min_w(px(0.))
                .cursor_pointer()
                .hover(|s| s.underline())
                .children(label.map(|l| div().flex_none().text_color(c(MUTED)).child(l)))
                .child(
                    div()
                        .flex_none()
                        .text_color(c(MUTED))
                        .child(format!("#{iid}")),
                )
                .child(
                    div()
                        .truncate()
                        .text_color(c(if state.eq_ignore_ascii_case("closed") {
                            MUTED
                        } else {
                            TEXT
                        }))
                        .child(title),
                )
                .on_click(move |_, window, cx| abromux_plugin::open_url(&url, window, cx))
        };
        let mut block = self.block_with("Work items", None, plus, r, cx);
        if !r.work_items.is_empty() {
            block = block.children(r.work_items.iter().map(|w| {
                let id = w.id.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w(px(0.)).child(row(
                        format!("wi-{}", w.iid),
                        Some(type_word(&w.link_type)),
                        w.iid,
                        w.title.clone(),
                        w.state.clone(),
                        w.web_url.clone(),
                    )))
                    .when(editable && !w.from_description, |d| {
                        d.child(
                            crate::edit::icon_button(
                                SharedString::from(format!("wi-remove-{}", w.iid)),
                                app_icon("close", 12., c(MUTED)),
                                "Remove",
                            )
                            .size(px(22.))
                            .on_click(cx.listener(
                                move |v, _, window, cx| v.unlink_work_item(id.clone(), window, cx),
                            )),
                        )
                    })
            }));
        } else if r.closes.is_empty() {
            block = block.child(div().text_color(c(MUTED)).child("None"));
        } else {
            block = block.children(r.closes.iter().map(|i| {
                row(
                    format!("closes-{}", i.iid),
                    None,
                    i.iid,
                    i.title.clone(),
                    i.state.clone(),
                    i.web_url.clone(),
                )
            }));
        }
        if self.more.wi_open {
            let pick = self.more.wi_type;
            let choice = |t: &'static str| {
                let on = pick == t;
                div()
                    .id(SharedString::from(format!("wi-type-{t}")))
                    .px_2()
                    .py(px(2.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(c(if on { HEADING } else { MUTED }))
                    .when(on, |d| d.bg(ca(0x89888d55)))
                    .child(type_word(t))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.more.wi_type = t;
                        cx.notify();
                    }))
            };
            block = block.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_2()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Link work item"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(choice("CLOSES"))
                            .child(choice("RELATED"))
                            .child(choice("MENTIONED")),
                    )
                    .child(
                        div()
                            .p_1()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(c(BORDER))
                            .bg(c(BG))
                            .child(Input::new(&self.more.wi_input).appearance(false)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(crate::edit::primary("gl-wi-link", "Add", true).on_click(
                                cx.listener(|v, _, window, cx| v.link_work_item(false, window, cx)),
                            ))
                            .child(
                                crate::edit::secondary("gl-wi-create", "Create new item").on_click(
                                    cx.listener(|v, _, window, cx| {
                                        v.link_work_item(true, window, cx)
                                    }),
                                ),
                            )
                            .child(crate::edit::secondary("gl-wi-cancel", "Cancel").on_click(
                                cx.listener(|v, _, _, cx| {
                                    v.more.wi_open = false;
                                    cx.notify();
                                }),
                            )),
                    ),
            );
        }
        block
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_issue_references() {
        assert_eq!(super::issue_iid("#12"), Some(12));
        assert_eq!(super::issue_iid("7"), Some(7));
        assert_eq!(
            super::issue_iid("http://gl/a/b/-/issues/34#note_1"),
            Some(34)
        );
        assert_eq!(super::issue_iid("http://gl/a/b/-/work_items/5"), Some(5));
        assert_eq!(super::issue_iid("nope"), None);
    }
}
