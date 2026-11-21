//! The head pipeline's reports, as GitLab shows them: on the Overview, the
//! Code Quality widget ("Code Quality scans found 2 new findings") and the
//! Test summary ("1 failed, 6 total tests"), each opening to its details;
//! and the Reports tab, the reports listed on the left and the one picked
//! on the right (test suites with their failed cases and traces, the
//! findings). A finding's place opens the file on Changes.

#[allow(unused_imports)]
use crate::style::px;
use crate::{review::Review, style::*, types::*, view::GitLabView};
use gpui::{prelude::*, *};

/// Which report the Reports tab shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pick {
    #[default]
    Quality,
    Tests,
}

/// What the reports' widgets and tab keep in a tab.
#[derive(Default)]
pub struct ReportsState {
    pub pick: Pick,
    /// The Overview's widgets opened.
    pub quality_open: bool,
    pub tests_open: bool,
    /// Test cases whose trace is shown, by "suite/class/name".
    pub traces: std::collections::HashSet<String>,
}

fn severity_color(s: &str) -> Hsla {
    match s.to_ascii_uppercase().as_str() {
        "BLOCKER" | "CRITICAL" => c(RED),
        "MAJOR" => c(0xe9731f),
        "MINOR" => c(ORANGE),
        _ => c(LINK),
    }
}

fn title_case(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut c = lower.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// GitLab's words for a test summary.
pub fn tests_line(t: &TestReport) -> String {
    let mut parts = vec![];
    if t.failed_count > 0 {
        parts.push(format!("{} failed", t.failed_count));
    }
    if t.error_count > 0 {
        parts.push(format!("{} errors", t.error_count));
    }
    if parts.is_empty() {
        parts.push("no failed tests".into());
    }
    format!(
        "Test summary: {}, {} total test{}",
        parts.join(", "),
        t.total_count,
        if t.total_count == 1 { "" } else { "s" }
    )
}

/// GitLab's words for the Code Quality comparison.
pub fn quality_line(q: &Quality) -> String {
    match (q.new.len(), q.resolved.len()) {
        (0, 0) => "Code Quality hasn't changed.".into(),
        (n, 0) => format!(
            "Code Quality scans found {n} new finding{}.",
            if n == 1 { "" } else { "s" }
        ),
        (0, f) => format!(
            "Code Quality improved: {f} finding{} fixed.",
            if f == 1 { "" } else { "s" }
        ),
        (n, f) => format!(
            "Code Quality scans found {n} new finding{} and {f} fixed.",
            if n == 1 { "" } else { "s" }
        ),
    }
}

impl GitLabView {
    /// Open `path` on Changes, at `line` (GitLab's "in src/a.rs:14").
    pub(crate) fn open_finding(&mut self, path: &str, line: Option<u32>, cx: &mut Context<Self>) {
        let Ok(fi) = self.file_index(path) else {
            return;
        };
        self.set_section(crate::Section::Changes, cx);
        if let Some(n) = line {
            self.selected_line = Some((fi, None, Some(n)));
        }
        self.scroll_to_file(fi, cx);
        cx.notify();
    }

    fn findings(&self, list: &[Finding], fixed: bool, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .children(list.iter().enumerate().map(|(i, f)| {
                let (path, line) = (f.file_path.clone(), f.line);
                div()
                    .flex()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(c(BG))
                    .child(
                        div()
                            .mt(px(5.))
                            .size(px(10.))
                            .flex_none()
                            .rounded_full()
                            .bg(if fixed {
                                c(GREEN)
                            } else {
                                severity_color(&f.severity)
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_color(c(TEXT)).child(format!(
                                "{}{} - {}",
                                if fixed { "Fixed: " } else { "" },
                                title_case(&f.severity),
                                f.description
                            )))
                            .child(
                                div()
                                    .id(SharedString::from(format!("finding-{fixed}-{i}")))
                                    .text_size(px(12.))
                                    .text_color(c(LINK))
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    .child(match line {
                                        Some(l) => format!("in {path}:{l}"),
                                        None => format!("in {path}"),
                                    })
                                    .on_click(cx.listener(move |v, _, _, cx| {
                                        v.open_finding(&path, line, cx)
                                    })),
                            ),
                    )
            }))
    }

    fn failed_cases(&self, t: &TestReport, cx: &mut Context<Self>) -> Div {
        let bad: Vec<(&TestSuite, &TestCase)> = t
            .test_suites
            .iter()
            .flat_map(|s| s.test_cases.iter().map(move |c| (s, c)))
            .filter(|(_, c)| c.status == "failed" || c.status == "error")
            .collect();
        div()
            .flex()
            .flex_col()
            .children(bad.into_iter().map(|(s, tc)| {
                let key = format!("{}/{}/{}", s.name, tc.classname, tc.name);
                let open = self.reports.traces.contains(&key);
                let trace = tc
                    .stack_trace
                    .clone()
                    .or_else(|| tc.system_output.clone())
                    .unwrap_or_default();
                let k = key.clone();
                div()
                    .flex()
                    .flex_col()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(c(BG))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon("minus-circle-filled", 14., c(RED)))
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(c(TEXT))
                                    .child(format!("{} {}", tc.classname, tc.name)),
                            )
                            .when(!trace.is_empty(), |d| {
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("trace-{key}")))
                                        .text_size(px(12.))
                                        .text_color(c(LINK))
                                        .cursor_pointer()
                                        .child(if open { "Hide details" } else { "View details" })
                                        .on_click(cx.listener(move |v, _, _, cx| {
                                            if !v.reports.traces.remove(&k) {
                                                v.reports.traces.insert(k.clone());
                                            }
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .when(open && !trace.is_empty(), |d| {
                        d.child(
                            div()
                                .mt_2()
                                .p_2()
                                .rounded(px(6.))
                                .bg(c(BG))
                                .font_family(self.mono(cx))
                                .text_size(px(12.))
                                .text_color(c(TEXT))
                                .children(trace.lines().map(|l| div().child(l.to_string()))),
                        )
                    })
            }))
    }

    /// The Overview's report widgets (none without reports).
    pub(crate) fn report_widgets(&self, r: &Review, cx: &mut Context<Self>) -> Vec<Div> {
        let mut out = vec![];
        let widget = |id: &'static str,
                      ok: bool,
                      line: String,
                      open: bool,
                      cx: &mut Context<Self>,
                      toggle: fn(&mut ReportsState)| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_3()
                .cursor_pointer()
                .child(icon(
                    if ok {
                        "check-circle-filled"
                    } else {
                        "minus-circle-filled"
                    },
                    18.,
                    c(if ok { GREEN } else { ORANGE }),
                ))
                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(line))
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-full")))
                        .text_color(c(TEXT))
                        .cursor_pointer()
                        .child("Full report")
                        .on_click(cx.listener(move |v, _, _, cx| {
                            cx.stop_propagation();
                            v.reports.pick = if id == "gl-w-tests" {
                                Pick::Tests
                            } else {
                                Pick::Quality
                            };
                            v.set_section(crate::Section::Reports, cx);
                        })),
                )
                .child(app_icon(
                    if open { "chevron-up" } else { "chevron-down" },
                    16.,
                    c(TEXT),
                ))
                .on_click(cx.listener(move |v, _, _, cx| {
                    toggle(&mut v.reports);
                    v.rebuild_rows(cx);
                    cx.notify();
                }))
        };
        let card = || {
            div()
                .flex()
                .flex_col()
                .rounded(px(12.))
                .overflow_hidden()
                .bg(c(RAISED))
                .text_color(c(TEXT))
        };
        if let Some(q) = &r.reports.quality {
            let open = self.reports.quality_open;
            out.push(
                card()
                    .child(widget(
                        "gl-w-quality",
                        q.new.is_empty(),
                        quality_line(q),
                        open,
                        cx,
                        |s| s.quality_open = !s.quality_open,
                    ))
                    .when(open, |d| {
                        d.child(self.findings(&q.new, false, cx))
                            .child(self.findings(&q.resolved, true, cx))
                    }),
            );
        }
        if let Some(t) = &r.reports.tests {
            let open = self.reports.tests_open;
            let ok = t.failed_count == 0 && t.error_count == 0;
            out.push(
                card()
                    .child(widget("gl-w-tests", ok, tests_line(t), open, cx, |s| {
                        s.tests_open = !s.tests_open
                    }))
                    .when(open, |d| d.child(self.failed_cases(t, cx))),
            );
        }
        out
    }

    /// The Reports tab.
    pub(crate) fn reports_page(&self, r: &Review, cx: &mut Context<Self>) -> AnyElement {
        let rep = &r.reports;
        if rep.count() == 0 {
            return div()
                .px_4()
                .py_6()
                .flex()
                .justify_center()
                .text_color(c(MUTED))
                .child("No reports for the latest pipeline.")
                .into_any_element();
        }
        let pick = match (self.reports.pick, &rep.quality, &rep.tests) {
            (Pick::Quality, None, _) => Pick::Tests,
            (Pick::Tests, _, None) => Pick::Quality,
            (p, ..) => p,
        };
        let item = |id: &'static str, label: &'static str, ok: bool, p: Pick| {
            let on = pick == p;
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .rounded(px(6.))
                .cursor_pointer()
                .when(on, |d| d.bg(c(RAISED)))
                .hover(|s| s.bg(c(RAISED)))
                .child(icon(
                    if ok {
                        "check-circle-filled"
                    } else {
                        "minus-circle-filled"
                    },
                    16.,
                    c(if ok { GREEN } else { ORANGE }),
                ))
                .child(div().text_color(c(TEXT)).child(label))
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.reports.pick = p;
                    cx.notify();
                }))
        };
        let left = div()
            .w(px(220.))
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .pr_3()
            .border_r_1()
            .border_color(c(RAISED))
            .when_some(rep.quality.as_ref(), |d, q| {
                d.child(item(
                    "gl-r-quality",
                    "Code quality",
                    q.new.is_empty(),
                    Pick::Quality,
                ))
            })
            .when_some(rep.tests.as_ref(), |d, t| {
                d.child(item(
                    "gl-r-tests",
                    "Test summary",
                    t.failed_count + t.error_count == 0,
                    Pick::Tests,
                ))
            });
        let right = match pick {
            Pick::Quality => {
                let q = rep.quality.as_ref().expect("picked");
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(quality_line(q)),
                    )
                    .child(
                        div()
                            .rounded(px(8.))
                            .overflow_hidden()
                            .bg(c(RAISED))
                            .child(self.findings(&q.new, false, cx))
                            .child(self.findings(&q.resolved, true, cx)),
                    )
            }
            Pick::Tests => {
                let t = rep.tests.as_ref().expect("picked");
                let mono = self.mono(cx);
                let cell = |s: String, w: f32| div().w(px(w)).flex_none().child(s);
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(tests_line(t)))
                    .child(div().text_color(c(MUTED)).child(format!(
                        "{} passed · {} failed · {} errors · {} skipped · {:.2}s",
                        t.success_count,
                        t.failed_count,
                        t.error_count,
                        t.skipped_count,
                        t.total_time
                    )))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .rounded(px(8.))
                            .overflow_hidden()
                            .bg(c(RAISED))
                            .child(
                                div()
                                    .flex()
                                    .px_4()
                                    .py_2()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(div().flex_1().child("Suite"))
                                    .child(cell("Duration".into(), 90.))
                                    .child(cell("Failed".into(), 70.))
                                    .child(cell("Errors".into(), 70.))
                                    .child(cell("Skipped".into(), 70.))
                                    .child(cell("Passed".into(), 70.)),
                            )
                            .children(t.test_suites.iter().map(|s| {
                                div()
                                    .flex()
                                    .px_4()
                                    .py_2()
                                    .border_t_1()
                                    .border_color(c(BG))
                                    .child(
                                        div()
                                            .flex_1()
                                            .font_family(mono.clone())
                                            .child(s.name.clone()),
                                    )
                                    .child(cell(format!("{:.2}s", s.total_time), 90.))
                                    .child(
                                        cell(s.failed_count.to_string(), 70.)
                                            .when(s.failed_count > 0, |d| d.text_color(c(RED))),
                                    )
                                    .child(cell(s.error_count.to_string(), 70.))
                                    .child(cell(s.skipped_count.to_string(), 70.))
                                    .child(cell(s.success_count.to_string(), 70.))
                            })),
                    )
                    .when(t.failed_count + t.error_count > 0, |d| {
                        d.child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Failed tests"),
                        )
                        .child(
                            div()
                                .rounded(px(8.))
                                .overflow_hidden()
                                .bg(c(RAISED))
                                .child(self.failed_cases(t, cx)),
                        )
                    })
            }
        };
        div()
            .id("gl-reports")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .py_4()
            .flex()
            .gap_4()
            .text_color(c(TEXT))
            .child(left)
            .child(div().flex_1().min_w(px(0.)).child(right))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn words_as_gitlab_has_them() {
        let t = crate::types::TestReport {
            total_count: 6,
            failed_count: 1,
            ..Default::default()
        };
        assert_eq!(
            super::tests_line(&t),
            "Test summary: 1 failed, 6 total tests"
        );
        let q = crate::types::Quality {
            new: vec![Default::default(), Default::default()],
            resolved: vec![],
        };
        assert_eq!(
            super::quality_line(&q),
            "Code Quality scans found 2 new findings."
        );
    }
}
