#!/usr/bin/env python3
"""Seed the local GitLab with fixtures for the review UI.

Creates users alice (author) and bob (reviewer), a project `alice/review-fixtures`
with a repo, and merge requests that cover what the review UI must handle:

  !1  "Add rate limiter"        modified, added, deleted, renamed files, a binary,
                                inline threads on new/old lines, a resolved thread,
                                a reply chain, a suggestion, a general comment, approved by bob;
                                two versions (pushed twice: the README change is in version 2)
  !2  "Draft: big refactor"     draft, one 3000-line file (scroll / perf)
  !3  "Fix typo"                one-line change, no discussion (the empty state)
  !4  "Warmer icon"             a changed PNG (image previews)
  !5  "Add a changelog"         merged
  !6  "An idea we dropped"      closed
  !7  "abromux 0.18.1 to 0.18.3" real code from this repo, with a thread
  !8  "Add 40 modules"          many files (the file tree)

Stdlib only. Rerunning deletes and recreates the project.
"""
import argparse, json, os, shutil, subprocess, sys, tempfile, time, urllib.error, urllib.parse, urllib.request

P = argparse.ArgumentParser()
P.add_argument("--url", default="http://localhost:8929")
P.add_argument("--token", default="glpat-abromux-dev-root-token")
A = P.parse_args()
API = A.url + "/api/v4"
PROJECT = "alice/review-fixtures"
PW = "abromux-dev-Pa55"
RICH = """### Review notes

Mostly **good**, a few _small_ things and one ~~big~~ medium one:

1. `shortcuts.json` lookup: see [the XDG spec](https://specifications.freedesktop.org/basedir-spec/latest/).
2. The OSC 52 path skips the size check.
   - nested point
   - another

> Quoting the guide: ⌘-click opens a link in a new terminal-browser tab.

| Key | Action |
| --- | ------ |
| ⌘F | find |
| ⌥⌘I | devtools |

- [x] tested on macOS 15
- [ ] tested with a remote shell

```rust
fn size_ok(n: usize) -> bool {
    n <= 1 << 20 // 1 MiB, like Ghostty
}
```
"""
# !7's base and head: abromux releases 0.18.0 and 0.18.3.
REAL = ("96368fb", "d89eb73")


def call(method, path, data=None, sudo=None, ok404=False):
    body = json.dumps(data).encode() if data is not None else None
    req = urllib.request.Request(API + path, data=body, method=method)
    req.add_header("PRIVATE-TOKEN", A.token)
    req.add_header("Content-Type", "application/json")
    if sudo:
        req.add_header("Sudo", sudo)
    try:
        with urllib.request.urlopen(req) as r:
            raw = r.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        if ok404 and e.code == 404:
            return None
        sys.exit(f"{method} {path} -> {e.code}: {e.read().decode()[:400]}")


def user(name, full):
    found = call("GET", f"/users?username={name}")
    if found:
        return found[0]
    return call("POST", "/users", {
        "username": name, "name": full, "email": f"{name}@example.test",
        "password": PW, "skip_confirmation": True,
    })


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def write(root, rel, text):
    p = os.path.join(root, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w" if isinstance(text, str) else "wb") as f:
        f.write(text)


MAIN_RS = """use std::collections::HashMap;

mod config;

fn main() {
    let cfg = config::load();
    let mut hits: HashMap<String, u32> = HashMap::new();
    for line in std::io::stdin().lines().map_while(Result::ok) {
        *hits.entry(line).or_default() += 1;
    }
    println!("{} distinct keys, limit {}", hits.len(), cfg.limit);
}
"""
CONFIG_RS = """pub struct Config {
    pub limit: u32,
}

pub fn load() -> Config {
    Config { limit: 100 }
}
"""


def png(w, h, color):
    """A real PNG: a `color(x, y) -> (r, g, b)` image, stdlib only."""
    import struct, zlib
    raw = b"".join(b"\0" + b"".join(bytes(color(x, y)) for x in range(w)) for y in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def seed_repo(dir, url):
    git(dir, "init", "-b", "main")
    git(dir, "config", "user.name", "Alice"); git(dir, "config", "user.email", "alice@example.test")
    write(dir, "README.md", "# review-fixtures\n\nA tiny project for testing review UIs.\n")
    write(dir, "src/main.rs", MAIN_RS)
    write(dir, "src/config.rs", CONFIG_RS)
    write(dir, "src/legacy.rs", "// old code nobody uses\npub fn unused() {}\n")
    write(dir, "docs/notes.txt", "Some notes.\nThey will move.\n")
    write(dir, "src/big.rs", "".join(f"pub fn f{i}() -> u32 {{ {i} }}\n" for i in range(3000)))
    write(dir, "TYPO.md", "This is teh readme.\n")
    write(dir, "assets/icon.png", png(48, 48, lambda x, y: (40, 90 + y * 3, 200)))
    # Pipelines for every push. There's no runner, so they stay pending: still
    # a status for the pipeline widget to show.
    write(dir, ".gitlab-ci.yml", "test:\n  script:\n    - cargo test\n")
    git(dir, "add", "-A"); git(dir, "commit", "-m", "Initial commit")
    git(dir, "remote", "add", "origin", url)
    git(dir, "push", "-u", "origin", "main")

    # !1: a mix of change kinds
    git(dir, "checkout", "-b", "rate-limiter")
    write(dir, "src/main.rs", MAIN_RS.replace("mod config;", "mod config;\nmod limiter;")
          .replace("        *hits.entry(line).or_default() += 1;",
                   "        if !limiter.allow(&line) {\n            continue;\n        }\n        *hits.entry(line).or_default() += 1;")
          .replace("    let mut hits", "    let mut limiter = limiter::Limiter::new(cfg.limit);\n    let mut hits"))
    write(dir, "src/limiter.rs", """use std::collections::HashMap;

/// Allows each key at most `limit` times.
pub struct Limiter {
    limit: u32,
    seen: HashMap<String, u32>,
}

impl Limiter {
    pub fn new(limit: u32) -> Self {
        Self { limit, seen: HashMap::new() }
    }

    pub fn allow(&mut self, key: &str) -> bool {
        let n = self.seen.entry(key.to_string()).or_insert(0);
        *n += 1;
        *n <= self.limit
    }
}
""")
    write(dir, "src/config.rs", CONFIG_RS.replace("limit: 100", "limit: std::env::var(\"LIMIT\").ok().and_then(|v| v.parse().ok()).unwrap_or(100)"))
    os.remove(os.path.join(dir, "src/legacy.rs"))
    git(dir, "mv", "docs/notes.txt", "docs/NOTES.md")
    write(dir, "assets/logo.png", bytes(range(256)) * 4)
    git(dir, "add", "-A"); git(dir, "commit", "-m", "Add a per-key rate limiter")
    write(dir, "README.md", "# review-fixtures\n\nA tiny project for testing review UIs.\n\nSet `LIMIT` to change the limit.\n")
    git(dir, "commit", "-am", "Document LIMIT", "-m", "The limit was only in the code. Say in the README how to\nchange it, with the default.")
    # Only the first commit for now: main() pushes the second after opening
    # the merge request, which makes it a second version (the version picker).
    git(dir, "push", "origin", "HEAD~1:refs/heads/rate-limiter")

    # !2: one huge change
    git(dir, "checkout", "main"); git(dir, "checkout", "-b", "big-refactor")
    write(dir, "src/big.rs", "".join(f"pub fn f{i}() -> u64 {{ {i} * 2 }}\n" for i in range(3000)))
    git(dir, "commit", "-am", "Double everything"); git(dir, "push", "-u", "origin", "big-refactor")

    # !3: one line
    git(dir, "checkout", "main"); git(dir, "checkout", "-b", "fix-typo")
    write(dir, "TYPO.md", "This is the readme.\n")
    git(dir, "commit", "-am", "Fix typo"); git(dir, "push", "-u", "origin", "fix-typo")

    # !5 and !6: one to merge, one to close
    for branch, path in (("changelog", "CHANGELOG.md"), ("abandoned", "IDEA.md")):
        git(dir, "checkout", "main"); git(dir, "checkout", "-b", branch)
        write(dir, path, f"# {path}\n")
        git(dir, "add", "-A"); git(dir, "commit", "-m", f"Add {path}"); git(dir, "push", "-u", "origin", branch)

    # !7: real code, abromux's own 0.18.0 → 0.18.3 (src, docs, skills), on a
    # base branch of its own. Skipped outside an abromux checkout.
    repo = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
    def snapshot(rev):
        for f in os.listdir(dir):
            if f not in (".git",):
                p = os.path.join(dir, f)
                shutil.rmtree(p) if os.path.isdir(p) else os.remove(p)
        tar = subprocess.run(["git", "-C", repo, "archive", rev, "src", "docs", "skills"],
                             capture_output=True, check=True).stdout
        subprocess.run(["tar", "-x", "-C", dir], input=tar, check=True)
    if subprocess.run(["git", "-C", repo, "cat-file", "-e", REAL[0]], capture_output=True).returncode == 0:
        git(dir, "checkout", "--orphan", "abromux-0.18.0")
        snapshot(REAL[0])
        git(dir, "add", "-A"); git(dir, "commit", "-m", "abromux 0.18.0"); git(dir, "push", "-u", "origin", "abromux-0.18.0")
        git(dir, "checkout", "-b", "abromux-0.18.3")
        snapshot(REAL[1])
        git(dir, "add", "-A"); git(dir, "commit", "-m", "abromux 0.18.1 to 0.18.3"); git(dir, "push", "-u", "origin", "abromux-0.18.3")

    # !4: a changed image
    git(dir, "checkout", "main"); git(dir, "checkout", "-b", "new-icon")
    write(dir, "assets/icon.png", png(48, 48, lambda x, y: (230, 120 + x * 2, 40)))
    git(dir, "commit", "-am", "Warmer icon"); git(dir, "push", "-u", "origin", "new-icon")

    # !8: many files, for the file tree (scrolling, folding, search)
    git(dir, "checkout", "main"); git(dir, "checkout", "-b", "many-files")
    for area in ("api", "cli", "core", "web"):
        for i in range(10):
            write(dir, f"{area}/mod{i}/lib.rs", f"pub fn {area}_{i}() -> u32 {{\n    {i}\n}}\n")
    git(dir, "add", "-A"); git(dir, "commit", "-m", "Add 40 modules"); git(dir, "push", "-u", "origin", "many-files")


def wait_diff(pid, iid, head=None):
    """The MR's diff_refs once GitLab has them (for `head`, if given)."""
    for _ in range(60):
        mr = call("GET", f"/projects/{pid}/merge_requests/{iid}")
        refs = mr.get("diff_refs") or {}
        if refs.get("head_sha") and (head is None or refs["head_sha"] == head):
            return refs
        time.sleep(1)
    sys.exit(f"MR !{iid} never got diff_refs")


def thread(pid, iid, refs, who, body, path, new_line=None, old_line=None, old_path=None):
    pos = {"position_type": "text", "base_sha": refs["base_sha"], "start_sha": refs["start_sha"],
           "head_sha": refs["head_sha"], "new_path": path, "old_path": old_path or path}
    if new_line: pos["new_line"] = new_line
    if old_line: pos["old_line"] = old_line
    return call("POST", f"/projects/{pid}/merge_requests/{iid}/discussions",
                {"body": body, "position": pos}, sudo=who)


def main():
    alice, bob = user("alice", "Alice Author"), user("bob", "Bob Reviewer")
    # GitLab only marks a deleted project for deletion; a second DELETE with
    # permanently_remove removes it, so old copies don't linger in "All open".
    # That includes copies earlier runs left marked.
    olds = [p for p in call("GET", "/projects?search=review-fixtures&per_page=100&simple=true")
            if p["path_with_namespace"].startswith(PROJECT)]
    for old in olds:
        if "deletion_scheduled" not in old["path_with_namespace"]:
            call("DELETE", f"/projects/{old['id']}")
        marked = call("GET", f"/projects/{old['id']}", ok404=True)
        if marked:
            q = urllib.parse.urlencode({"permanently_remove": "true", "full_path": marked["path_with_namespace"]})
            call("DELETE", f"/projects/{old['id']}?{q}")
        for _ in range(60):  # deletion is async
            if not call("GET", f"/projects/{old['id']}", ok404=True):
                break
            time.sleep(1)
    if olds:
        time.sleep(2)
    proj = call("POST", "/projects", {"name": "review-fixtures", "path": "review-fixtures",
                                      "visibility": "private", "initialize_with_readme": False}, sudo="alice")
    pid = proj["id"]
    call("POST", f"/projects/{pid}/members", {"user_id": bob["id"], "access_level": 30})

    host = urllib.parse.urlparse(A.url)
    push_url = f"{host.scheme}://root:{A.token}@{host.netloc}/{PROJECT}.git"
    # Labels as GitLab colors them, a scoped one among them.
    for name, color in (("backend", "#1068bf"), ("bug", "#dd2b0e"), ("priority::high", "#e9be74"), ("docs", "#2da160")):
        call("POST", f"/projects/{pid}/labels", {"name": name, "color": color})
    mk = lambda src, title, desc, labels="": call("POST", f"/projects/{pid}/merge_requests", {
        "source_branch": src, "target_branch": "main", "title": title, "description": desc, "labels": labels,
        "reviewer_ids": [bob["id"]], "assignee_id": alice["id"]}, sudo="alice")
    with tempfile.TemporaryDirectory() as d:
        seed_repo(d, push_url)
        m1 = mk("rate-limiter", "Add rate limiter",
                "Adds a per-key limiter.\n\n- [x] limiter\n- [ ] metrics\n\n```rust\nlet l = Limiter::new(3);\n```", "backend,priority::high")
        wait_diff(pid, m1["iid"])
        # The second push: !1's version 2.
        git(d, "push", "origin", "rate-limiter")
        head = subprocess.run(["git", "rev-parse", "rate-limiter"], cwd=d, capture_output=True, text=True).stdout.strip()
    m2 = mk("big-refactor", "Draft: big refactor", "Touches 3000 lines.")
    mk("fix-typo", "Fix typo", "", "docs,bug")
    mk("new-icon", "Warmer icon", "Makes the icon orange.")
    m5 = mk("changelog", "Add a changelog", "")
    m6 = mk("abandoned", "An idea we dropped", "")
    wait_diff(pid, m5["iid"])
    # GitLab checks mergeability in the background: wait for it.
    for _ in range(60):
        st = call("GET", f"/projects/{pid}/merge_requests/{m5['iid']}")["detailed_merge_status"]
        if st == "mergeable":
            break
        time.sleep(1)
    call("PUT", f"/projects/{pid}/merge_requests/{m5['iid']}/merge", {"should_remove_source_branch": True}, sudo="alice")
    call("PUT", f"/projects/{pid}/merge_requests/{m6['iid']}", {"state_event": "close"}, sudo="alice")
    branches = [b["name"] for b in call("GET", f"/projects/{pid}/repository/branches?per_page=100")]
    if "abromux-0.18.3" in branches:
        m7 = call("POST", f"/projects/{pid}/merge_requests", {
            "source_branch": "abromux-0.18.3", "target_branch": "abromux-0.18.0",
            "title": "abromux 0.18.1 to 0.18.3",
            "description": "Real code to review: OSC 52 clipboard, safer ⌘-click, ⌘F in terminal-browser tabs.",
            "reviewer_ids": [bob["id"]], "assignee_id": alice["id"]}, sudo="alice")
        refs7 = wait_diff(pid, m7["iid"])
        diffs = call("GET", f"/projects/{pid}/merge_requests/{m7['iid']}/diffs?per_page=100")
        # A thread on the first added line of terminal_browser.rs.
        tb = next((d for d in diffs if d["new_path"] == "src/terminal_browser.rs"), None)
        if tb:
            line = next(int(h.split("+")[1].split(",")[0].split(" ")[0])
                        for h in tb["diff"].splitlines() if h.startswith("@@"))
            for raw in tb["diff"].splitlines()[1:]:
                if raw.startswith("+"):
                    break
                if not raw.startswith("-"):
                    line += 1
            thread(pid, m7["iid"], refs7, "bob", "Could this share the address bar's find logic?",
                   "src/terminal_browser.rs", new_line=line)
        # Markdown of every kind, for comparing how comments render.
        call("POST", f"/projects/{pid}/merge_requests/{m7['iid']}/notes", {"body": RICH}, sudo="bob")
    mk("many-files", "Add 40 modules", "Lots of small files.")

    refs = wait_diff(pid, m1["iid"], head)
    t1 = thread(pid, m1["iid"], refs, "bob", "Should this be `u64`? Keys can repeat a lot.", "src/limiter.rs", new_line=6)
    call("POST", f"/projects/{pid}/merge_requests/{m1['iid']}/discussions/{t1['id']}/notes",
         {"body": "u32 is plenty here, the limit caps it."}, sudo="alice")
    call("POST", f"/projects/{pid}/merge_requests/{m1['iid']}/discussions/{t1['id']}/notes",
         {"body": "Fair enough :+1:"}, sudo="bob")
    thread(pid, m1["iid"], refs, "bob",
           "Simpler:\n\n```suggestion:-0+0\n        *n <= self.limit // inclusive\n```", "src/limiter.rs", new_line=17)
    t3 = thread(pid, m1["iid"], refs, "bob", "Was anything still calling this?", "src/legacy.rs", old_line=2)
    call("PUT", f"/projects/{pid}/merge_requests/{m1['iid']}/discussions/{t3['id']}?resolved=true", sudo="alice")
    thread(pid, m1["iid"], refs, "bob", "Parse error is silently ignored.", "src/config.rs", new_line=6)
    call("POST", f"/projects/{pid}/merge_requests/{m1['iid']}/notes", {"body": "Looks good overall, two nits. :thumbsup:"}, sudo="bob")
    call("POST", f"/projects/{pid}/merge_requests/{m1['iid']}/approve", {}, sudo="bob")

    wait_diff(pid, m2["iid"])
    print(f"seeded {A.url}/{PROJECT}/-/merge_requests  (users alice/bob, password {PW})")


main()
