//! GitLab's "New merge request", from a project's list: the source and
//! target branches (picked from the project's, as you type), title,
//! description, and "Mark as draft", "Delete source branch when merge
//! request is accepted" and "Squash commits". Creating it opens it.

#[allow(unused_imports)]
use crate::style::px;
use crate::{style::*, types::Branch, view::GitLabView};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputState};

/// The form's fields, made once per tab.
pub struct NewMr {
    /// Open, for this project.
    pub project: Option<u64>,
    pub source: Entity<InputState>,
    pub target: Entity<InputState>,
    pub title: Entity<InputState>,
    pub description: Entity<InputState>,
    pub draft: bool,
    pub remove_source: bool,
    pub squash: bool,
    /// The project's branches, once fetched.
    pub branches: Vec<Branch>,
    /// Which branch field the suggestions are for (true: target).
    pub picking_target: bool,
    pub busy: bool,
    pub error: Option<String>,
}

impl NewMr {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        let one = |p: &'static str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).placeholder(p))
        };
        NewMr {
            project: None,
            source: one("Source branch", window, cx),
            target: one("Target branch", window, cx),
            title: one("Title", window, cx),
            description: cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(true)
                    .auto_grow(4, 16)
                    .placeholder(
                        "Describe the goal of the changes and what reviewers should be aware of.",
                    )
            }),
            draft: false,
            remove_source: true,
            squash: false,
            branches: vec![],
            picking_target: false,
            busy: false,
            error: None,
        }
    }

    pub fn fields(&self) -> [&Entity<InputState>; 4] {
        [&self.source, &self.target, &self.title, &self.description]
    }
}

/// A title from a branch name, as GitLab suggests one: `fix-the-thing`
/// → "Fix the thing".
pub fn title_from_branch(b: &str) -> String {
    let last = b.rsplit('/').next().unwrap_or(b);
    let words = last.replace(['-', '_'], " ");
    let mut c = words.trim().chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

impl GitLabView {
    /// Open the form for the project being browsed (or close it).
    pub(crate) fn open_new_mr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.loc.browse.as_ref().map(|b| b.id).filter(|id| *id != 0) else {
            return;
        };
        if self.new_mr.project.is_some() {
            self.new_mr.project = None;
            cx.notify();
            return;
        }
        self.new_mr.project = Some(p);
        self.new_mr.error = None;
        for b in self.new_mr.fields() {
            b.update(cx, |b, cx| b.set_value("", window, cx));
        }
        self.new_mr.source.update(cx, |b, cx| b.focus(window, cx));
        let Some(c) = self.client.clone() else { return };
        cx.spawn_in(window, async move |this, cx| {
            let got = cx
                .background_spawn(async move {
                    let branches = c.branches(p, "")?;
                    anyhow::Ok(branches)
                })
                .await;
            let _ = this.update_in(cx, |v, window, cx| {
                match got {
                    Ok(bs) => {
                        if let Some(d) = bs.iter().find(|b| b.default) {
                            let name = d.name.clone();
                            v.new_mr
                                .target
                                .update(cx, |b, cx| b.set_value(name, window, cx));
                        }
                        v.new_mr.branches = bs;
                    }
                    Err(e) => v.new_mr.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Create it, then open it.
    pub(crate) fn create_mr(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(p), Some(c)) = (self.new_mr.project, self.client.clone()) else {
            return;
        };
        let v = |b: &Entity<InputState>, cx: &App| b.read(cx).value().trim().to_string();
        let (source, target) = (v(&self.new_mr.source, cx), v(&self.new_mr.target, cx));
        let mut title = v(&self.new_mr.title, cx);
        if title.is_empty() {
            title = title_from_branch(&source);
        }
        if source.is_empty() || target.is_empty() || title.is_empty() {
            self.new_mr.error =
                Some("A source branch, a target branch and a title, please.".into());
            cx.notify();
            return;
        }
        let title = crate::edit::draft_title(&title, self.new_mr.draft);
        let description = self.new_mr.description.read(cx).value().to_string();
        let (remove, squash) = (self.new_mr.remove_source, self.new_mr.squash);
        self.new_mr.busy = true;
        self.new_mr.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let got = cx
                .background_spawn(async move {
                    c.create_mr_with(p, &source, &target, &title, &description, remove, squash)
                })
                .await;
            let _ = this.update_in(cx, |v, _, cx| {
                v.new_mr.busy = false;
                match got {
                    Ok(mr) => {
                        v.new_mr.project = None;
                        v.open_mr(mr.project_id, mr.iid, cx);
                    }
                    Err(e) => v.new_mr.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The list's "New merge request", when a project's list is shown.
    pub(crate) fn new_mr_button(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let browsing = self.loc.browse.as_ref().is_some_and(|b| b.id != 0);
        (browsing && self.me.is_some()).then(|| {
            crate::edit::primary("gl-new-mr", "New merge request", true)
                .on_click(cx.listener(|v, _, window, cx| v.open_new_mr(window, cx)))
        })
    }

    /// The form, in place of the list while it's open.
    pub(crate) fn new_mr_form(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        self.new_mr.project?;
        let n = &self.new_mr;
        let description = self.md_editor(
            crate::compose::MdBox::NewMr,
            Input::new(&n.description)
                .appearance(false)
                .into_any_element(),
            window,
            cx,
        );

        let field = |label: &'static str, input: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(TEXT))
                        .child(label),
                )
                .child(
                    div()
                        .p_2()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(c(BORDER))
                        .bg(c(CARD))
                        .child(input),
                )
        };
        // Branches matching what's typed in the field being filled.
        let typed = if n.picking_target {
            &n.target
        } else {
            &n.source
        }
        .read(cx)
        .value()
        .to_lowercase();
        let suggestions: Vec<String> = n
            .branches
            .iter()
            .filter(|b| typed.is_empty() || b.name.to_lowercase().contains(&typed))
            .filter(|b| b.name.to_lowercase() != typed)
            .take(12)
            .map(|b| b.name.clone())
            .collect();
        let target_side = n.picking_target;
        let chips = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(suggestions.into_iter().map(|name| {
                let pick = name.clone();
                div()
                    .id(SharedString::from(format!("gl-branch-{name}")))
                    .px_2()
                    .py(px(1.))
                    .rounded(px(6.))
                    .bg(ca(0x89888d33))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .font_family(self.mono(cx))
                    .hover(|s| s.bg(ca(0x89888d66)))
                    .child(name)
                    .on_click(cx.listener(move |v, _, window, cx| {
                        let b = if target_side {
                            v.new_mr.target.clone()
                        } else {
                            v.new_mr.source.clone()
                        };
                        let pick = pick.clone();
                        if !target_side && v.new_mr.title.read(cx).value().trim().is_empty() {
                            let t = title_from_branch(&pick);
                            v.new_mr
                                .title
                                .update(cx, |b, cx| b.set_value(t, window, cx));
                        }
                        b.update(cx, |b, cx| b.set_value(pick, window, cx));
                        cx.notify();
                    }))
            }));
        let side_tab = |id: &'static str, label: &'static str, on: bool, target: bool| {
            div()
                .id(id)
                .text_size(px(12.))
                .px_2()
                .rounded(px(6.))
                .cursor_pointer()
                .text_color(c(if on { HEADING } else { MUTED }))
                .when(on, |d| d.bg(ca(0x89888d40)))
                .child(label)
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.new_mr.picking_target = target;
                    cx.notify();
                }))
        };
        let tick =
            |id: &'static str, on: bool, label: &'static str, f: fn(&mut crate::create::NewMr)| {
                crate::edit::checkbox(id, on, label).on_click(cx.listener(move |v, _, _, cx| {
                    f(&mut v.new_mr);
                    cx.notify();
                }))
            };
        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .py_4()
                .max_w(px(900.))
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(c(HEADING))
                        .child("New merge request"),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(div().flex_1().child(field(
                            "Source branch",
                            Input::new(&n.source).appearance(false).into_any_element(),
                        )))
                        .child(div().flex_1().child(field(
                            "Target branch",
                            Input::new(&n.target).appearance(false).into_any_element(),
                        ))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(c(MUTED))
                                .child("Branches for"),
                        )
                        .child(side_tab("gl-pick-source", "source", !target_side, false))
                        .child(side_tab("gl-pick-target", "target", target_side, true)),
                )
                .child(chips)
                .child(field(
                    "Title",
                    Input::new(&n.title).appearance(false).into_any_element(),
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(TEXT))
                                .child("Description"),
                        )
                        .child(
                            div()
                                .p_2()
                                .rounded(px(8.))
                                .border_1()
                                .border_color(c(BORDER))
                                .bg(c(CARD))
                                .child(description),
                        ),
                )
                .child(tick("gl-new-draft", n.draft, "Mark as draft", |n| {
                    n.draft = !n.draft
                }))
                .child(tick(
                    "gl-new-remove",
                    n.remove_source,
                    "Delete source branch when merge request is accepted.",
                    |n| n.remove_source = !n.remove_source,
                ))
                .child(tick(
                    "gl-new-squash",
                    n.squash,
                    "Squash commits when merge request is accepted.",
                    |n| n.squash = !n.squash,
                ))
                .children(n.error.clone().map(|e| div().text_color(c(RED)).child(e)))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            crate::edit::primary(
                                "gl-new-create",
                                if n.busy {
                                    "Creating…"
                                } else {
                                    "Create merge request"
                                },
                                !n.busy,
                            )
                            .when(!n.busy, |d| {
                                d.on_click(cx.listener(|v, _, window, cx| v.create_mr(window, cx)))
                            }),
                        )
                        .child(crate::edit::secondary("gl-new-cancel", "Cancel").on_click(
                            cx.listener(|v, _, _, cx| {
                                v.new_mr.project = None;
                                cx.notify();
                            }),
                        )),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn titles_from_branches() {
        assert_eq!(super::title_from_branch("fix-the-thing"), "Fix the thing");
        assert_eq!(
            super::title_from_branch("zil/add_rate_limit"),
            "Add rate limit"
        );
    }
}
