# GitLab tab: parity with GitLab's web UI

Every control on GitLab 19.5's merge request pages (overview, changes,
commits, pipelines, reports, a merged merge request) and its merge request
list, read off the live dev GitLab (`dev/gitlab`), with where the GitLab tab
has it and how that was checked.

How it was checked:

- **live**: a test against the dev GitLab (`ABROMUX_GITLAB_LIVE=1 cargo test -p abromux-gitlab`; the file is named);
- **app**: done through the running tab (`dev/gitlab/app.sh`, its `surface.plugin` command) and the result read back from GitLab's API;
- **shot**: a screenshot of the tab next to GitLab's own page;
- **stand-in**: the feature needs a paid GitLab (Premium, Ultimate) or GitLab Duo, which the dev GitLab (CE) doesn't have: checked against GitLab's documented API through a local proxy answering those calls, and its absence on CE checked live.

## Header

| GitLab | Tab | Checked |
|---|---|---|
| Title, state badge, author, source → target, copy branch name | ✓ | shot |
| Code ▾: Check out branch, Open in Web IDE, Download Patches / Plain diff | ✓ | shot |
| Edit (title, description, target branch, delete source branch, squash) | ✓ | app (`edit_mr`), live (`manage.rs`) |
| Add a to-do item / Mark as done | ✓ | live (`actions.rs`), app |
| Notifications on / off | ✓ | live, app |
| ⋮ Mark as draft / ready, Close / Reopen, Lock discussion, Copy reference, Report abuse | ✓ (Report abuse opens GitLab's form: no API) | live, app |
| Tabs Overview, Commits, Pipelines, Reports, Changes with counts | ✓ | shot |
| Threads counter with previous / next open thread | ✓ (`n` / `p` too) | shot |
| Your review ▾: Comment / Approve / Request changes, summary, Submit / Continue, pending comments | ✓ | live (`actions.rs`), app |
| Stacked merge requests | ✓ | earlier work |

## Overview

| GitLab | Tab | Checked |
|---|---|---|
| Description with task lists (clickable), code blocks with copy | ✓ | app (`task`), shot |
| 👍 👎 Add reaction | ✓ | earlier work |
| Pipeline widget | ✓ | shot |
| Approval widget, Approve / Revoke | ✓ | live |
| Approval rules (Premium) | ✓ | stand-in |
| Code Quality widget, Test summary widget, Full report | ✓ | live (`reports.rs`, after `dev/gitlab/reports.sh`), shot |
| Merge widget: Merge / Set to auto-merge, Delete source branch, Squash, Edit commit message | ✓ | live (`manage.rs`) |
| Expand merge checks (each check's status) | ✓ | live (`actions.rs`), shot |
| Rebase source branch, commits behind | ✓ | earlier work |
| Merge train: Add to merge train, train status, Remove (Premium) | ✓ | stand-in |
| Merge request dependencies (Premium) | ✓ | stand-in |
| Merged: Revert / Cherry-pick (to a branch or a new merge request) | ✓ | live (`manage.rs`) |
| Activity: All activity / comments / history, sort | ✓ | shot |
| History notes incl. title changes (GitLab's HTML) | ✓ | shot |
| Thread: Resolve, Reply, Edit, Delete, Copy link, Add reaction, Collapse replies | ✓ | earlier work |
| Create issue to resolve thread | ✓ | live (`actions.rs`) |
| Apply suggestion (with message), Add suggestion to batch | ✓ | live (`actions.rs`), shot |
| Comment box: Comment / Start thread ▾, Start a review, internal note | ✓ | live, app |
| Editor: Write / Preview, Text style, bold, italic, strikethrough, quote, code, link, lists, checklist, table, attach file, quick actions, comment templates | ✓ (Markdown with Preview; GitLab's optional WYSIWYG "rich text" mode isn't reproduced) | live (quick actions, templates), app (attach), shot |
| `/` quick action autocomplete, `@` mentions | ✓ | shot, earlier work |

## Sidebar

| GitLab | Tab | Checked |
|---|---|---|
| Assignee, Reviewer (with review states), Labels, Milestone: Edit dropdowns | ✓ | earlier work |
| Re-request review | ✓ | app |
| Request review from GitLab Duo (Duo) | ✓ | stand-in |
| Work items: linked items with relationship, + Link work item / Create new item, remove | ✓ | live (`actions.rs`) |
| Time tracking: Add time entry, Set estimate | ✓ | app |
| Participants | ✓ | shot |

## Changes

| GitLab | Tab | Checked |
|---|---|---|
| File browser (tree / list, search, hide), compare versions | ✓ | earlier work |
| Expand / collapse all, Inline / Side-by-side, one file at a time | ✓ | earlier work |
| Show whitespace changes | ✓ | app, shot |
| File header: copy path, Viewed, Comment on this file, Show options (View file @ sha, Copy link to the file, Hide comments) | ✓ | app, shot |
| Comment on a line, on several lines, on an image point | ✓ | app, shot |
| Expand unchanged lines | ✓ | earlier work |
| Rapid Diffs (beta) | — GitLab's own renderer switch; the tab's diff is always virtualized | — |
| Large merge requests | better: every file (GitLab stops at 1000 and collapses the rest), ~same first load, 0.14 s from cache | timed through `dev/gitlab/slow-proxy.py` |

## Commits, Pipelines, Reports

| GitLab | Tab | Checked |
|---|---|---|
| Commits by day, Toggle commit description, Copy commit SHA, a commit's changes | ✓ | earlier work |
| Add previously merged commits | ✓ | live (`actions.rs`) |
| Pipelines: status, stages per job status, Retry / Cancel, Run pipeline | ✓ | shot, live (`manage.rs`) |
| Failed jobs with Retry, Download artifacts | ✓ | shot, live (`reports.rs`) |
| Reports tab: Code quality, Test summary (suites, failed tests and traces) | ✓ | live (`reports.rs`), shot |

## GitLab Duo (Duo-licensed GitLab)

| GitLab | Tab | Checked |
|---|---|---|
| Summarize code changes (new merge request), Generate commit message, Summarize review, Duo Chat about the merge request | ✓ | stand-in |

GitLab doesn't document which global id type Duo's actions expect for a
merge request; the tab sends `gid://gitlab/MergeRequest/<id>`.

## Merge request list

| GitLab | Tab | Checked |
|---|---|---|
| State tabs with counts, sort and direction | ✓ | earlier work |
| Search and filter tokens (author, assignee, reviewer, label, milestone, draft, branches, negation) | ✓ | unit tests, shot |
| Recent searches | ✓ | shot |
| Bulk edit (state, labels, milestone, assignee, subscription) | ✓ | app |
| Export as CSV, Subscribe to RSS feed | ✓ (the CSV is written by the tab; GitLab mails it) | unit test |
| New merge request | ✓ | app |
| Pagination (20 per page) | better: the whole list, scrolled | — |
