//! A pipeline's jobs on the Pipelines tab, as GitLab's list has them: each
//! stage's status icon (its jobs' worst), "Failed jobs" opening to them
//! with Retry, and "Download artifacts" listing the jobs that kept some.
//! Fetched when the tab shows.

#[allow(unused_imports)]
use crate::style::px;
use crate::{
    review::Review,
    style::*,
    types::*,
    view::{GitLabView, Load},
};
use gpui::{prelude::*, *};
use std::collections::HashMap;

/// The tab's jobs, per pipeline, and what's open.
#[derive(Default)]
pub struct JobsState {
    pub jobs: HashMap<u64, Load<Vec<Job>>>,
    pub failed_open: std::collections::HashSet<u64>,
    pub artifacts_open: Option<u64>,
}

/// How bad a job's status is, worst highest: a stage shows its worst.
fn badness(status: &str) -> u8 {
    match status {
        "failed" => 6,
        "running" => 5,
        "pending" | "created" | "waiting_for_resource" | "preparing" | "scheduled" => 4,
        "manual" => 3,
        "canceled" => 2,
        "skipped" => 1,
        _ => 0,
    }
}

/// The stages in order, each with its worst status.
pub fn stages(jobs: &[Job]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for j in jobs.iter().rev() {
        match out.iter_mut().find(|(s, _)| *s == j.stage) {
            Some((_, st)) if badness(&j.status) > badness(st) => *st = j.status.clone(),
            Some(_) => {}
            None => out.push((j.stage.clone(), j.status.clone())),
        }
    }
    out
}

fn status_icon(status: &str, size: f32) -> Svg {
    let p = Pipeline {
        status: status.to_string(),
        ..Default::default()
    };
    pipeline_icon(&p, size)
}

impl GitLabView {
    /// Fetch the jobs of the pipelines the tab lists, once each.
    pub(crate) fn want_jobs(&mut self, r: &Review, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.client.clone() else { return };
        let project = r.mr.project_id;
        for p in r.pipelines.iter().take(20) {
            if self.jobs.jobs.contains_key(&p.id) {
                continue;
            }
            self.jobs.jobs.insert(p.id, Load::Loading);
            let (c, id) = (c.clone(), p.id);
            cx.spawn_in(window, async move |this, cx| {
                let got = cx
                    .background_spawn(async move { c.jobs(project, id) })
                    .await;
                let _ = this.update(cx, |v, cx| {
                    v.jobs.jobs.insert(
                        id,
                        match got {
                            Ok(j) => Load::Ready(j),
                            Err(_) => Load::Ready(vec![]),
                        },
                    );
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// Retry job `id`; the pipeline's jobs are fetched again after.
    pub(crate) fn retry_job(
        &mut self,
        pipeline: u64,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.jobs.jobs.remove(&pipeline);
        self.write(
            crate::edit::Write::Pipeline,
            None,
            move |c, p, _| c.retry_job(p, id),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// The Stages column: one icon per stage.
    pub(crate) fn stage_icons(&self, p: &Pipeline) -> Div {
        match self.jobs.jobs.get(&p.id) {
            Some(Load::Ready(jobs)) if !jobs.is_empty() => {
                div()
                    .flex()
                    .gap_1()
                    .children(stages(jobs).into_iter().map(|(stage, st)| {
                        let tip: SharedString = format!("{stage}: {st}").into();
                        div()
                            .id(SharedString::from(format!("stage-{}-{stage}", p.id)))
                            .child(status_icon(&st, 20.))
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                            })
                    }))
            }
            _ => div().child(pipeline_icon(p, 20.)),
        }
    }

    /// Under a pipeline: "Failed jobs" and "Download artifacts".
    pub(crate) fn pipeline_jobs(&self, p: &Pipeline, cx: &mut Context<Self>) -> Option<Div> {
        let Some(Load::Ready(jobs)) = self.jobs.jobs.get(&p.id) else {
            return None;
        };
        let failed: Vec<&Job> = jobs.iter().filter(|j| j.status == "failed").collect();
        let kept: Vec<&Job> = jobs
            .iter()
            .filter(|j| j.artifacts.iter().any(|a| a.file_type == "archive"))
            .collect();
        if failed.is_empty() && kept.is_empty() {
            return None;
        }
        let pid = p.id;
        let open = self.jobs.failed_open.contains(&pid);
        let can = self.me.is_some();
        let menu_open = self.jobs.artifacts_open == Some(pid);
        let downloads: Vec<(String, String)> = kept
            .iter()
            .map(|j| (j.name.clone(), format!("{}/artifacts/download", j.web_url)))
            .collect();
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .pb_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .when(!failed.is_empty(), |d| {
                            d.child(
                                div()
                                    .id(SharedString::from(format!("failed-jobs-{pid}")))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .cursor_pointer()
                                    .text_color(c(LINK))
                                    .child(app_icon(
                                        if open {
                                            "chevron-down"
                                        } else {
                                            "chevron-right"
                                        },
                                        12.,
                                        c(LINK),
                                    ))
                                    .child(format!("Failed jobs {}", failed.len()))
                                    .on_click(cx.listener(move |v, _, _, cx| {
                                        if !v.jobs.failed_open.remove(&pid) {
                                            v.jobs.failed_open.insert(pid);
                                        }
                                        cx.notify();
                                    })),
                            )
                        })
                        .when(!downloads.is_empty(), |d| {
                            d.child(
                                div()
                                    .relative()
                                    .child(
                                        crate::edit::secondary(
                                            SharedString::from(format!("artifacts-{pid}")),
                                            "Download artifacts",
                                        )
                                        .h(px(26.))
                                        .gap_1()
                                        .child(icon("download", 14., c(TEXT)))
                                        .on_click(
                                            cx.listener(move |v, _, _, cx| {
                                                v.jobs.artifacts_open =
                                                    if v.jobs.artifacts_open == Some(pid) {
                                                        None
                                                    } else {
                                                        Some(pid)
                                                    };
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .when(menu_open, |d| {
                                        d.child(
                                            deferred(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "artifacts-menu-{pid}"
                                                    )))
                                                    .absolute()
                                                    .top(px(30.))
                                                    .left_0()
                                                    .min_w(px(220.))
                                                    .py_1()
                                                    .rounded(px(8.))
                                                    .border_1()
                                                    .border_color(c(BORDER))
                                                    .bg(c(CARD))
                                                    .shadow_lg()
                                                    .occlude()
                                                    .on_mouse_down_out(cx.listener(
                                                        |v, _, _, cx| {
                                                            v.jobs.artifacts_open = None;
                                                            cx.notify();
                                                        },
                                                    ))
                                                    .children(downloads.iter().enumerate().map(
                                                        |(i, (name, url))| {
                                                            let url = url.clone();
                                                            div()
                                                                .id(SharedString::from(format!(
                                                                    "artifact-{pid}-{i}"
                                                                )))
                                                                .px_3()
                                                                .py_1()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(c(RAISED)))
                                                                .text_color(c(TEXT))
                                                                .child(format!("{name}:archive"))
                                                                .on_click(cx.listener(
                                                                    move |v, _, window, cx| {
                                                                        v.jobs.artifacts_open =
                                                                            None;
                                                                        abromux_plugin::open_url(
                                                                            &url, window, cx,
                                                                        );
                                                                    },
                                                                ))
                                                        },
                                                    )),
                                            )
                                            .with_priority(2),
                                        )
                                    }),
                            )
                        }),
                )
                .when(open, |d| {
                    d.children(failed.into_iter().map(|j| {
                        let (id, url) = (j.id, j.web_url.clone());
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded(px(6.))
                            .bg(c(CARD))
                            .child(status_icon(&j.status, 16.))
                            .child(
                                div()
                                    .w(px(160.))
                                    .truncate()
                                    .text_color(c(TEXT))
                                    .child(j.name.clone()),
                            )
                            .child(
                                div()
                                    .w(px(120.))
                                    .truncate()
                                    .text_color(c(MUTED))
                                    .child(j.stage.clone()),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("job-{id}")))
                                    .flex_1()
                                    .text_color(c(LINK))
                                    .cursor_pointer()
                                    .child(format!("#{id}"))
                                    .on_click(move |_, window, cx| {
                                        abromux_plugin::open_url(&url, window, cx)
                                    }),
                            )
                            .when(can, |d| {
                                d.child(
                                    crate::edit::secondary(
                                        SharedString::from(format!("retry-job-{id}")),
                                        "Retry",
                                    )
                                    .h(px(26.))
                                    .on_click(cx.listener(
                                        move |v, _, window, cx| v.retry_job(pid, id, window, cx),
                                    )),
                                )
                            })
                    }))
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_stage_shows_its_worst_job() {
        let j = |s: &str, st: &str| crate::types::Job {
            stage: s.into(),
            status: st.into(),
            ..Default::default()
        };
        let s = super::stages(&[
            j("test", "success"),
            j("test", "failed"),
            j("build", "success"),
        ]);
        assert_eq!(
            s,
            vec![
                ("build".to_string(), "success".to_string()),
                ("test".to_string(), "failed".to_string())
            ]
        );
    }
}
