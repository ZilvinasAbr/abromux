# Developing the GitLab plugin

The plugin (`crates/abromux-gitlab`) is built against a real GitLab running in
Docker, with merge requests made to cover what a review UI has to handle. Each
change is checked three ways: tests on recorded API responses, the same tests
against the live GitLab, and screenshots of the tab in a running abromux,
compared with GitLab's own pages.

## The GitLab

```bash
dev/gitlab/up.sh            # start (first boot ~5 min), make tokens, seed
dev/gitlab/up.sh --no-seed  # start only
dev/gitlab/up.sh --down     # stop; data is kept (docker compose -f dev/gitlab/compose.yml down -v wipes it)
python3 dev/gitlab/seed.py  # recreate the fixtures project from scratch
```

- <http://localhost:8929>, GitLab CE pinned in `compose.yml`.
- Users `root`, `alice` (author) and `bob` (reviewer, dark mode), password `abromux-dev-Pa55`.
- Tokens: `glpat-abromux-dev-root-token` (admin, `api` + `sudo`, for seeding) and
  `glpat-abromux-dev-bob-token` (bob, `api`, what the app uses; it writes too).
- `alice/review-fixtures` (see `seed.py`):
  - **!1** every kind of file change, threads on new and old lines, a reply chain, a
    resolved thread, a suggestion, approval, two versions;
  - **!2** a draft with a 3000-line change;
  - **!3** one line, no comments;
  - **!4** a changed PNG;
  - **!5** merged and **!6** closed;
  - **!7** real code: abromux's own 0.18.0 → 0.18.3, with a thread;
  - **!8** 40 small files (the file tree);
  - **!9** past GitLab's diff limits: 1,203 files and ~70,000 lines, a
    40,000-line file and a 300 KB one-line one (`tests/big.rs`).
- Labels `backend`, `bug`, `priority::high`, `docs` and open milestones `v1.0`,
  `v1.1`, for the sidebar's pickers.
- Every push runs a pipeline. There's no runner, so they stay pending and
  GitLab fails them as stuck after an hour: both states get shown.

## Tests

```bash
cargo test -p abromux-gitlab                         # offline, on tests/fixtures
ABROMUX_GITLAB_LIVE=1 cargo test -p abromux-gitlab   # also against the live GitLab (tests/manage.rs merges
                                                     # a merge request it makes, as alice, with a token it revokes)
dev/gitlab/record-fixtures.sh                        # re-record tests/fixtures after reseeding
```

## A GitLab far away

The Docker GitLab answers in milliseconds, so it hides what a remote one
costs. `slow-proxy.py` puts it behind what a server across the internet
charges: each new connection waits for its handshakes (150 ms), each request
for its round trip (120 ms), and connections stay open between requests.
`load-time.sh` times the list and some reviews through it, over the socket:

```bash
dev/gitlab/slow-proxy.py &                    # http://localhost:8930
dev/gitlab/load-time.sh                       # list, then !7, !1 and !9
ABROMUX_GITLAB_TIMING=1 dev/gitlab/load-time.sh   # and the review's phases in the app's log
```

Mind that this GitLab runs its web server in one process (`compose.yml`), so
it answers one request at a time; a real one runs several and takes a
batch's requests together.

## The app

`build.sh` builds abromux on a copy of gpui that can render its own window
to a PNG (`gpui-capture.py`, only active when `GPUI_CAPTURE_DIR` is set), so
screenshots work with the screen locked or the window hidden. It goes to
`target/capture/`; the normal build and `Cargo.lock` are untouched.

`app.sh` runs it apart from any other AbroMux: its own control socket, holder
socket and no session, signed in as bob.

```bash
dev/gitlab/build.sh
dev/gitlab/app.sh start
dev/gitlab/app.sh rpc surface.create '{"type":"gitlab","url":"http://localhost:8929/alice/review-fixtures/-/merge_requests/1/diffs"}'
dev/gitlab/app.sh rpc surface.plugin '{"surface":"surface:2","command":"state"}'
dev/gitlab/app.sh snap /tmp/changes.png
dev/gitlab/app.sh stop
```

The tab's commands (`surface.plugin`) do what clicks do and answer with what
it shows: `state`, `open {url | iid}`, `section {name}`, `file {path}`,
`fold {path}`, `viewed {path}`, `tree`, `parallel {on}`, `expand {path, hunk, lines}`,
`compare {base, head}`, `next_thread`, `prev_thread`, `thread {body}`, `search {query}`,
`list {scope, state}`, `browse {path | id}` (a project's merge requests; `{}`: the projects),
`paste {text}` (into the list's search), `back`, `refresh`; and writing: `merge {auto, squash, remove_source_branch}`,
`cancel_auto_merge`, `close`, `reopen`, `draft {on}`, `edit_mr {title, description, save}`, `edit_note {id, body, save}`,
`delete_note {id, sure}`, `pipeline {id, action: retry|cancel|run}`, `menu {on}` (the header's ⋮). Real input goes through `debug.input`
(`key:n`, `ui:down,X,Y;ui:up,X,Y`, `ui:scroll,X,Y,DY`).

The loop in one command, tests + build + a screenshot in `target/shots/`:

```bash
dev/gitlab/iterate.sh changes http://localhost:8929/alice/review-fixtures/-/merge_requests/1/diffs \
  '{"command":"file","args":{"path":"src/limiter.rs"}}'
dev/gitlab/iterate.sh --no-build overview http://localhost:8929/alice/review-fixtures/-/merge_requests/1
dev/gitlab/scroll-perf.sh   # frame times while scrolling the open tab
dev/gitlab/restore-test.sh  # quit and relaunch: the tab comes back where it was
dev/gitlab/signin-test.sh   # a rejected token, then signing in again from the tab
dev/gitlab/refresh-test.sh  # a new reply arrives on refresh, the view stays put (reseed after)
```

Nothing here writes to your Keychain: `app.sh` sets `ABROMUX_GITLAB_NO_KEYCHAIN`.

For the reference, sign in to <http://localhost:8929> as bob and open the same
page, at about the same width.
