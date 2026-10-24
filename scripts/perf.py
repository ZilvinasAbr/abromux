#!/usr/bin/env python3
"""abromux performance suite: measure, record every run, compare with history.

    scripts/perf.py run              build, measure, record, compare (~1.5 min)
    scripts/perf.py run --headless   only the parts that need no window (~10 s)
    scripts/perf.py run --full       longer idle windows and more startup runs
    scripts/perf.py compare [ID]     compare a recorded run (default: latest)
    scripts/perf.py history [METRIC] recent runs, or one metric over time
    scripts/perf.py report [--md]    HTML (or Markdown) report with charts
    scripts/perf.py install-hook     run the headless suite before every push

Every run is appended to a JSON-lines history shared by all worktrees of this
repo (<git common dir>/abromux-perf/history.jsonl, or $ABROMUX_PERF_HISTORY) with
its commit, branch, dirty state and machine. A run is compared only with runs
from the same machine: per metric, against the median of the last N (default
10). It's a regression when it's worse by more than both the metric's
threshold and 3x the baseline's spread (MAD), so normal noise doesn't flag.
Runs made while the machine is busy (load > cores/2) are recorded but kept
out of baselines.

The app part launches abromux in its own socket/session/config, keeps it in
front (macOS throttles covered windows) and quits it. Leave the Mac alone
while it runs. Python 3 stdlib only.
"""

import argparse
import datetime as dt
import hashlib
import json
import os
import random
import re
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# name: (unit, better, relative threshold, absolute noise floor, description)
METRICS = {
    "binary_size_mb":       ("MB",   "lower",  0.03, 0.2,  "release binary size"),
    "vt_ascii_mbps":        ("MB/s", "higher", 0.08, 0,    "headless VT parse, plain ASCII"),
    "vt_color_mbps":        ("MB/s", "higher", 0.08, 0,    "headless VT parse, truecolor SGR"),
    "vt_unicode_mbps":      ("MB/s", "higher", 0.08, 0,    "headless VT parse, CJK + emoji"),
    "startup_socket_ms":    ("ms",   "lower",  0.15, 20,   "launch -> control socket answers"),
    "startup_prompt_ms":    ("ms",   "lower",  0.15, 50,   "launch -> shell prompt on screen"),
    "idle_footprint_mb":    ("MB",   "lower",  0.10, 2,    "footprint, idle, 1 terminal"),
    "per_terminal_mb":      ("MB",   "lower",  0.25, 0.3,  "footprint per extra terminal"),
    "idle_cpu_pct":         ("%",    "lower",  0.50, 0.5,  "idle CPU, 1 terminal"),
    "idle_cpu_11_pct":      ("%",    "lower",  0.50, 0.5,  "idle CPU, 11 terminals"),
    "app_ascii_mbps":       ("MB/s", "higher", 0.10, 0,    "in-app throughput, plain ASCII"),
    "app_color_mbps":       ("MB/s", "higher", 0.10, 0,    "in-app throughput, truecolor SGR"),
    "app_unicode_mbps":     ("MB/s", "higher", 0.10, 0,    "in-app throughput, CJK + emoji"),
    "throughput_cpu_s":     ("s",    "lower",  0.15, 0.2,  "app CPU time for the throughput run"),
    "footprint_after_mb":   ("MB",   "lower",  0.10, 3,    "footprint after throughput"),
}

MODES = {
    # startup runs, settle seconds, idle CPU window seconds, throughput passes
    "quick": dict(startup_runs=3, settle=5, idle=10, passes=3),
    "full": dict(startup_runs=5, settle=15, idle=30, passes=3),
}


def sh(*cmd, check=True, **kw):
    return subprocess.run(cmd, check=check, capture_output=True, text=True, **kw).stdout.strip()


def log(*a):
    print(*a, file=sys.stderr, flush=True)


# ---- history -----------------------------------------------------------------

def history_path():
    if p := os.environ.get("ABROMUX_PERF_HISTORY"):
        return Path(p)
    common = Path(sh("git", "-C", str(ROOT), "rev-parse", "--git-common-dir"))
    if not common.is_absolute():
        common = ROOT / common
    return common.resolve() / "abromux-perf" / "history.jsonl"


def load_history():
    p = history_path()
    if not p.exists():
        return []
    return [json.loads(l) for l in p.read_text().splitlines() if l.strip()]


def append_history(rec):
    p = history_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    with p.open("a") as f:
        f.write(json.dumps(rec, separators=(",", ":")) + "\n")


def git_info():
    g = lambda *a: sh("git", "-C", str(ROOT), *a, check=False)
    return {
        "sha": g("rev-parse", "HEAD"),
        "branch": g("rev-parse", "--abbrev-ref", "HEAD"),
        "subject": g("log", "-1", "--format=%s"),
        # The suite's own history/output never lives in the tree, so any
        # change here is a code change.
        "dirty": bool(g("status", "--porcelain", "--untracked-files=no")),
    }


def machine_info():
    ctl = lambda k: sh("sysctl", "-n", k, check=False)
    m = {
        "model": ctl("hw.model"),
        "chip": ctl("machdep.cpu.brand_string"),
        "cores": int(ctl("hw.ncpu") or 0),
        "mem_gb": round(int(ctl("hw.memsize") or 0) / 2**30),
        "os": sh("sw_vers", "-productVersion", check=False),
        "power": "ac" if "AC Power" in sh("pmset", "-g", "batt", check=False) else "battery",
    }
    m["id"] = hashlib.sha1(f"{m['model']}|{m['chip']}|{m['mem_gb']}".encode()).hexdigest()[:10]
    return m


# ---- measurement helpers -------------------------------------------------------

def gen_data(d: Path):
    """Same files as scripts/bench.sh, so numbers line up with its results."""
    if (d / "unicode.txt").exists():
        return
    d.mkdir(parents=True, exist_ok=True)
    random.seed(1)
    words = "the quick brown fox jumps over lazy dog terminal emulator throughput benchmark".split()
    with open(d / "ascii.txt", "w") as f:
        for i in range(400_000):
            f.write(f"{i:07d} " + " ".join(random.choice(words) for _ in range(10)) + "\n")
    with open(d / "color.txt", "w") as f:
        for i in range(150_000):
            f.write("".join(f"\x1b[38;2;{random.randrange(256)};{random.randrange(256)};{random.randrange(256)}m{random.choice(words)} " for _ in range(8)) + "\x1b[0m\n")
    cjk = "終端模擬器の性能を測定する😀🚀✅日本語中文한국어"
    with open(d / "unicode.txt", "w") as f:
        for i in range(150_000):
            f.write("".join(random.choice(cjk) for _ in range(40)) + "\n")


def footprint_mb(pid):
    out = sh("footprint", "-p", str(pid), check=False)
    m = re.search(r"Footprint: ([\d.]+) ([KMG]B)", out)
    if not m:
        return None
    return float(m[1]) * {"KB": 1 / 1024, "MB": 1, "GB": 1024}[m[2]]


def cpu_secs(pid):
    t = sh("ps", "-o", "time=", "-p", str(pid), check=False)
    secs = 0.0
    for part in t.split(":"):
        secs = secs * 60 + float(part)
    return secs


def median(xs):
    return statistics.median(xs) if xs else None


# ---- the suite -------------------------------------------------------------------

class Suite:
    def __init__(self, binary: Path, mode: str, data: Path):
        self.bin = binary
        self.cfg = MODES[mode]
        self.data = data
        self.metrics = {}
        self.samples = {}

    def put(self, name, value, samples=None):
        if value is None:
            return
        self.metrics[name] = round(value, 3)
        if samples and len(samples) > 1:
            self.samples[name] = [round(s, 4) for s in samples]
        unit = METRICS[name][0]
        log(f"  {name:<20} {value:10.2f} {unit}")

    def binary_size(self):
        self.put("binary_size_mb", self.bin.stat().st_size / 1e6)

    def headless(self):
        log("headless VT parse")
        files = [str(self.data / f"{f}.txt") for f in ("ascii", "color", "unicode")]
        env = {**os.environ, "ABROMUX_BENCH_PASSES": "5"}
        out = sh(str(self.bin), "bench-vt", *files, env=env)
        runs = {}
        for line in out.splitlines():
            path, size, secs = line.rsplit(" ", 2)
            runs.setdefault(Path(path).stem, []).append(int(size) / 1e6 / float(secs))
        # Best pass: other load only ever slows a CPU-bound loop down.
        for f, mbps in runs.items():
            self.put(f"vt_{f}_mbps", max(mbps), mbps)

    # -- app --

    def app(self):
        tmp = Path(tempfile.mkdtemp(prefix="abromux-perf-"))
        xdg = tmp / "xdg"
        (xdg / "ghostty").mkdir(parents=True)
        # Ghostty's default font size, so the grid matches bench.sh runs.
        (xdg / "ghostty" / "config").write_text("font-size = 13\n")
        self.session = tmp / "session.json"
        self.env = {**os.environ, "ABROMUX_SOCKET_PATH": str(tmp / "abromux.sock"),
                    "ABROMUX_SESSION_PATH": str(self.session), "XDG_CONFIG_HOME": str(xdg)}
        self.proc = None
        try:
            self._app(tmp)
        finally:
            self.quit()
            shutil.rmtree(tmp, ignore_errors=True)

    def cli(self, *args, check=False):
        r = subprocess.run([str(self.bin), *args], env=self.env, capture_output=True, text=True, timeout=10)
        if check and r.returncode:
            raise RuntimeError(f"abromux {' '.join(args)}: {r.stderr.strip()}")
        return r

    def launch(self):
        self.session.write_text(json.dumps({"version": 1, "window": {"x": 364, "y": 200, "width": 1000, "height": 700}, "active": 0,
                                            "workspaces": [{"root": {"kind": "pane", "tabs": [{"kind": "terminal", "cwd": str(Path.home())}]}}]}))
        self.proc = subprocess.Popen([str(self.bin)], env=self.env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def quit(self):
        if not self.proc or self.proc.poll() is not None:
            return
        self.proc.send_signal(signal.SIGTERM)
        try:
            self.proc.wait(5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()

    def wait_prompt(self, timeout=6):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if re.search(r"[%$#] *$", self.cli("read-screen").stdout, re.M):
                return True
            time.sleep(0.02)
        return False

    def front(self):
        self.cli("focus-window", "--window", "window:1")

    def idle_cpu(self, secs):
        c0 = cpu_secs(self.proc.pid)
        time.sleep(secs)
        return (cpu_secs(self.proc.pid) - c0) / secs * 100

    def _app(self, tmp):
        c = self.cfg
        log("startup")
        sock, prompt = [], []
        for r in range(c["startup_runs"]):
            t0 = time.monotonic()
            self.launch()
            while self.cli("ping").returncode:
                if self.proc.poll() is not None:
                    raise RuntimeError("abromux exited during startup")
                time.sleep(0.01)
            sock.append((time.monotonic() - t0) * 1000)
            if self.wait_prompt():
                prompt.append((time.monotonic() - t0) * 1000)
            else:
                log("  (prompt not seen)")
            if r < c["startup_runs"] - 1:
                self.quit()
                time.sleep(1)
        self.put("startup_socket_ms", median(sock), sock)
        self.put("startup_prompt_ms", median(prompt), prompt)

        log("idle")
        pid = self.proc.pid
        self.front()
        time.sleep(c["settle"])
        self.put("idle_footprint_mb", footprint_mb(pid))
        self.put("idle_cpu_pct", self.idle_cpu(c["idle"]))

        self.front()
        m0 = footprint_mb(pid)
        for _ in range(10):
            self.cli("new-workspace", check=True)
        time.sleep(c["settle"])
        m1 = footprint_mb(pid)
        if m0 is not None and m1 is not None:
            self.put("per_terminal_mb", (m1 - m0) / 10)
        self.put("idle_cpu_11_pct", self.idle_cpu(c["idle"]))

        log("in-app throughput")
        # The shell times each cat up to the terminal's reply to a DA1 query
        # sent after it: the reply comes only once everything before is parsed.
        self.cli("new-workspace", check=True)
        self.front()
        time.sleep(2)
        self.wait_prompt()
        size_file, res = tmp / "size.txt", tmp / "throughput.txt"
        self.cli("send", f"stty size > {size_file}\\n")
        time.sleep(1)
        grid = size_file.read_text().strip() if size_file.exists() else "?"
        if grid in ("24 80", "?"):
            log(f"  WARNING: grid is {grid!r}; the window was probably not drawn, in-app numbers are suspect")
        script = tmp / "throughput.zsh"
        script.write_text(f"""zmodload zsh/datetime
old=$(stty -g)
stty -echo -icanon min 1
for i in {{1..{c['passes']}}}; do
  for f in ascii color unicode; do
    s=$EPOCHREALTIME
    cat {self.data}/$f.txt
    printf '\\e[c'
    read -r -d c reply
    print "$f $((EPOCHREALTIME - s))" >> {res}
  done
done
stty $old
print done >> {res}
clear
""")
        c0 = cpu_secs(pid)
        self.cli("send", f"zsh {script}\\n")
        end = time.monotonic() + 600
        while time.monotonic() < end:
            if res.exists() and "done" in res.read_text():
                break
            time.sleep(0.25)
        else:
            raise RuntimeError("in-app throughput timed out")
        self.put("throughput_cpu_s", cpu_secs(pid) - c0)
        times = {}
        for line in res.read_text().splitlines():
            if line != "done":
                f, t = line.split()
                times.setdefault(f, []).append(float(t))
        for f, ts in times.items():
            mb = (self.data / f"{f}.txt").stat().st_size / 1e6
            mbps = [mb / t for t in ts]
            self.put(f"app_{f}_mbps", median(mbps), mbps)
        self.put("footprint_after_mb", footprint_mb(pid))
        self.grid = grid


# ---- comparison ----------------------------------------------------------------------

def baseline_for(rec, history, n):
    """Earlier quiet runs from the same machine, most recent last."""
    earlier = [h for h in history if h["machine"]["id"] == rec["machine"]["id"] and h["timestamp"] < rec["timestamp"] and not h.get("busy")]
    return earlier[-n:]


def judge(name, cur, values):
    """-> (baseline median, delta fraction, status, allowed deviation) with
    status in ok/better/WORSE/new."""
    unit, better, rel, floor, _ = METRICS[name]
    if not values:
        return None, None, "new", None
    med = statistics.median(values)
    mad = statistics.median(abs(v - med) for v in values) * 1.4826 if len(values) >= 3 else 0
    allowed = max(rel * abs(med), floor, 3 * mad)
    diff = cur - med
    delta = diff / med if med else None
    worse = diff > allowed if better == "lower" else -diff > allowed
    improved = -diff > allowed if better == "lower" else diff > allowed
    return med, delta, "WORSE" if worse else "better" if improved else "ok", allowed


def rows_for(rec, history, n):
    """Per metric of `rec`: its value next to the baseline from `history`."""
    base = baseline_for(rec, history, n)
    prev = base[-1] if base else None
    rows = []
    for name, (unit, better, _rel, _floor, desc) in METRICS.items():
        if name not in rec["metrics"]:
            continue
        vals = [b["metrics"][name] for b in base if name in b["metrics"]]
        med, delta, status, allowed = judge(name, rec["metrics"][name], vals)
        rows.append({"name": name, "unit": unit, "better": better, "desc": desc, "now": rec["metrics"][name],
                     "baseline": med, "allowed": allowed, "delta": delta, "n": len(vals),
                     "prev": prev["metrics"].get(name) if prev else None, "status": status})
    return rows


def fmt_rec(r):
    g = r["git"]
    return f"{r['id']}  {r['timestamp'][:16].replace('T', ' ')}  {g['sha'][:8]}{'+' if g['dirty'] else ' '} {g['branch']}{'  [busy]' if r.get('busy') else ''}"


def compare(rec, history, n=10, out=sys.stdout):
    base = baseline_for(rec, history, n)
    prev = base[-1] if base else None
    print(f"\nrun  {fmt_rec(rec)}  ({rec['mode']}{', ' + rec['label'] if rec.get('label') else ''})", file=out)
    if prev:
        print(f"prev {fmt_rec(prev)}", file=out)
        oses = {b["machine"]["os"] for b in base}
        if oses != {rec["machine"]["os"]}:
            print(f"note: baseline spans macOS {', '.join(sorted(oses))}; this run is on {rec['machine']['os']}", file=out)
    if rec.get("busy"):
        print(f"note: machine was busy (load {rec['load']}); this run is kept out of future baselines", file=out)
    print(f"baseline: median of the last {len(base)} quiet run(s) on this machine ({rec['machine']['chip']}, {rec['machine']['mem_gb']} GB)\n", file=out)
    use_color = out.isatty() and not os.environ.get("NO_COLOR")
    paint = lambda s, c: f"\x1b[{c}m{s}\x1b[0m" if use_color else s
    print(f"{'metric':<20} {'now':>10} {'baseline':>10} {'Δ':>8} {'prev':>10}  {'unit':<5} status", file=out)
    regressions = []
    for row in rows_for(rec, history, n):
        name, unit, cur, med, delta, pv, status = (row[k] for k in ("name", "unit", "now", "baseline", "delta", "prev", "status"))
        cell = lambda v: f"{v:10.2f}" if v is not None else f"{'-':>10}"
        d = f"{delta * 100:+7.1f}%" if delta is not None else f"{'-':>8}"
        st = {"WORSE": paint("▼ WORSE", "31;1"), "better": paint("▲ better", "32"), "ok": "  ok", "new": "  new"}[status]
        if status == "ok" and row["n"] < 3:
            st += f" (n={row['n']})"
        print(f"{name:<20} {cell(cur)} {cell(med)} {d} {cell(pv)}  {unit:<5} {st}", file=out)
        if status == "WORSE":
            regressions.append(name)
    if regressions:
        print(paint(f"\n{len(regressions)} regression(s): {', '.join(regressions)}", "31;1"), file=out)
    elif base:
        print("\nno regressions", file=out)
    return regressions


SPARK = "▁▂▃▄▅▆▇█"


def spark(vals):
    lo, hi = min(vals), max(vals)
    return "".join(SPARK[int((v - lo) / (hi - lo) * 7)] if hi > lo else SPARK[3] for v in vals)


def show_history(history, metric, n):
    mid = machine_info()["id"]
    runs = [h for h in history if h["machine"]["id"] == mid][-n:]
    if not runs:
        print("no runs recorded on this machine yet")
        return
    if metric:
        if metric not in METRICS:
            sys.exit(f"unknown metric {metric}; one of: {', '.join(METRICS)}")
        unit, better, *_rest, desc = METRICS[metric]
        pts = [(r, r["metrics"][metric]) for r in runs if metric in r["metrics"]]
        if not pts:
            print(f"{metric}: no data")
            return
        print(f"{metric} ({desc}; {better} is better)  {spark([v for _, v in pts])}\n")
        for r, v in pts:
            print(f"{fmt_rec(r)}  {v:10.2f} {unit}  {r['git']['subject'][:50]}")
        return
    print("+ = uncommitted changes, * = busy machine (not in baselines)\n")
    cols = ["startup_socket_ms", "idle_footprint_mb", "vt_ascii_mbps", "vt_color_mbps", "app_ascii_mbps", "app_color_mbps"]
    print(f"{'id':<10} {'when':<16}  {'commit':<9} {'branch':<28} " + " ".join(f"{c[:12]:>12}" for c in cols))
    for r in runs:
        cells = " ".join(f"{r['metrics'][c]:12.1f}" if c in r["metrics"] else f"{'-':>12}" for c in cols)
        g = r["git"]
        print(f"{r['id']:<10} {r['timestamp'][:16].replace('T', ' '):<16}  {g['sha'][:8]}{'+' if g['dirty'] else ' '} {g['branch'][:27]:<27}{'*' if r.get('busy') else ' '} {cells}")
    print("\ntrends: " + "  ".join(f"{c} {spark(v)}" for c in cols if len(v := [r['metrics'][c] for r in runs if c in r['metrics']]) > 1))


# ---- commands ----------------------------------------------------------------------------

def load_avg():
    """Current 1-minute load average; sampled before, between and after the parts."""
    return round(os.getloadavg()[0], 1)


def binary_path():
    if b := os.environ.get("ABROMUX_BIN"):
        return Path(b)
    return Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release" / "abromux"


def cmd_run(a):
    if not a.no_build:
        log("cargo build --release")
        subprocess.run(["cargo", "build", "--release", "--quiet"], cwd=ROOT, check=True)
    binary = binary_path()
    if not binary.exists():
        sys.exit(f"{binary} not found; build it or set ABROMUX_BIN")
    data = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "perf-data"
    gen_data(data)

    mode = "headless" if a.headless else "full" if a.full else "quick"
    loads = [load_avg()]
    cores = os.cpu_count() or 1
    if loads[0] > cores / 2:
        log(f"WARNING: load average is {loads[0]} on {cores} cores; numbers will be noisy and the run won't count as a baseline")
    t0 = time.monotonic()
    suite = Suite(binary, "full" if a.full else "quick", data)
    suite.binary_size()
    suite.headless()
    loads.append(load_avg())
    if not a.headless:
        caffeinate = subprocess.Popen(["caffeinate", "-d", "-i"])
        try:
            suite.app()
        finally:
            caffeinate.terminate()
        loads.append(load_avg())

    now = dt.datetime.now(dt.timezone.utc)
    rec = {
        "id": now.strftime("%y%m%d%H%M%S"),
        "timestamp": now.isoformat(timespec="seconds"),
        "mode": mode,
        "label": a.label,
        "git": git_info(),
        "machine": machine_info(),
        "duration_s": round(time.monotonic() - t0, 1),
        "load": max(loads),
        "busy": max(loads) > cores / 2 or a.busy,
        "metrics": suite.metrics,
        "samples": suite.samples,
    }
    if getattr(suite, "grid", None):
        rec["grid"] = suite.grid
    history = load_history()
    if not a.dry_run:
        append_history(rec)
        log(f"recorded run {rec['id']} in {history_path()}")
    regressions = compare(rec, history, a.baseline)
    log("\nscripts/perf.py report  opens charts of all runs")
    if regressions and a.fail_on_regression:
        sys.exit(1)


def cmd_compare(a):
    history = load_history()
    if not history:
        sys.exit("no runs recorded yet")
    rec = history[-1] if a.id in (None, "latest") else next((h for h in history if h["id"] == a.id), None)
    if not rec:
        sys.exit(f"no run {a.id}")
    regs = compare(rec, history, a.baseline)
    sys.exit(1 if regs and a.fail_on_regression else 0)


def cmd_history(a):
    show_history(load_history(), a.metric, a.n)


def machine_runs(history, machine_id=None):
    mid = machine_id or machine_info()["id"]
    return [h for h in history if h["machine"]["id"] == mid]


def report_html(history, n):
    runs = machine_runs(history)
    if not runs:
        sys.exit("no runs recorded on this machine yet")
    latest = runs[-1]
    slim = []
    for r in runs:
        status = {row["name"]: row["status"] for row in rows_for(r, history, n)}
        slim.append({"id": r["id"], "timestamp": r["timestamp"], "mode": r["mode"], "label": r.get("label"),
                     "busy": bool(r.get("busy")), "load": r.get("load"), "git": r["git"], "os": r["machine"]["os"],
                     "metrics": r["metrics"], "status": status})
    data = {
        "generated": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
        "machine": latest["machine"],
        "history": str(history_path()),
        "baselineN": n,
        "metrics": {k: {"unit": u, "better": b, "desc": d} for k, (u, b, _r, _f, d) in METRICS.items()},
        "latest": {"id": latest["id"], "rows": rows_for(latest, history, n), "baseCount": len(baseline_for(latest, history, n))},
        "runs": slim,
    }
    template = (ROOT / "scripts" / "perf-report.html").read_text()
    # </ can't appear inside the inline <script>.
    return template.replace("/*DATA*/null", json.dumps(data).replace("</", "<\\/"))


def report_md(history, n, last=20):
    runs = machine_runs(history)
    if not runs:
        sys.exit("no runs recorded on this machine yet")
    latest, m = runs[-1], runs[-1]["machine"]
    g = latest["git"]
    out = [f"# abromux performance", "",
           f"{m['chip']}, {m['mem_gb']} GB, macOS {m['os']}. {len(runs)} runs recorded. Generated {dt.datetime.now():%Y-%m-%d %H:%M}.", "",
           f"## Latest run: {latest['id']}", "",
           f"`{g['sha'][:8]}`{' (uncommitted changes)' if g['dirty'] else ''} on `{g['branch']}`: {g['subject']}  ",
           f"{latest['timestamp'][:16].replace('T', ' ')} UTC, {latest['mode']}" + (f", {latest['label']}" if latest.get("label") else "")
           + (f". **Machine was busy (load {latest['load']}); not used as a baseline.**" if latest.get("busy") else ""), ""]
    rows = rows_for(latest, history, n)
    regs = [r["name"] for r in rows if r["status"] == "WORSE"]
    nbase = len(baseline_for(latest, history, n))
    out += [f"**{len(regs)} regression(s): {', '.join(regs)}**" if regs else ("No regressions." if nbase else "No earlier runs to compare with."),
            f"Baseline: median of the last {nbase} quiet runs.", "",
            f"| Metric | Now | Baseline | Δ | Status | Trend (last {last} quiet runs) |", "|---|---:|---:|---:|---|---|"]
    status_txt = {"WORSE": "🔴 worse", "better": "🟢 better", "ok": "ok", "new": "new"}
    for r in rows:
        trend = [h["metrics"][r["name"]] for h in runs[-last:] if r["name"] in h["metrics"] and not h.get("busy")]
        b = f"{r['baseline']:.2f}" if r["baseline"] is not None else "–"
        d = f"{r['delta'] * 100:+.1f}%" if r["delta"] is not None else "–"
        out.append(f"| {r['desc']} | {r['now']:.2f} {r['unit']} | {b} | {d} | {status_txt[r['status']]} | {spark(trend) if len(trend) > 1 else ''} |")
    cols = ["startup_socket_ms", "idle_footprint_mb", "vt_ascii_mbps", "vt_color_mbps", "app_ascii_mbps", "app_color_mbps"]
    out += ["", f"## Recent runs", "", "| Run | When (UTC) | Commit | Branch | " + " | ".join(cols) + " |",
            "|---|---|---|---|" + "---:|" * len(cols)]
    for r in reversed(runs[-last:]):
        g = r["git"]
        cells = " | ".join(f"{r['metrics'][c]:.1f}" if c in r["metrics"] else "–" for c in cols)
        flag = " (busy)" if r.get("busy") else ""
        out.append(f"| {r['id']}{flag} | {r['timestamp'][:16].replace('T', ' ')} | `{g['sha'][:8]}`{'+' if g['dirty'] else ''} | {g['branch']} | {cells} |")
    return "\n".join(out) + "\n"


def cmd_report(a):
    history = load_history()
    md = a.md or (a.out and a.out.endswith(".md"))
    text = report_md(history, a.baseline) if md else report_html(history, a.baseline)
    if a.out == "-":
        sys.stdout.write(text)
        return
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    path = Path(a.out) if a.out else target / ("perf-report.md" if md else "perf-report.html")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    print(path)
    if not md and not a.no_open:
        subprocess.run(["open", str(path)], check=False)


HOOK = """#!/bin/sh
# abromux perf: headless suite before each push (installed by scripts/perf.py install-hook).
# Skip with ABROMUX_PERF_SKIP=1 git push.
[ -n "$ABROMUX_PERF_SKIP" ] && exit 0
exec "$(git rev-parse --show-toplevel)/scripts/perf.py" run --headless --label pre-push {flags}
"""


def cmd_install_hook(a):
    hooks = Path(sh("git", "-C", str(ROOT), "rev-parse", "--git-common-dir"))
    hooks = (hooks if hooks.is_absolute() else ROOT / hooks) / "hooks" / "pre-push"
    if hooks.exists() and "abromux perf" not in hooks.read_text():
        sys.exit(f"{hooks} already exists and isn't ours; add `scripts/perf.py run --headless` to it yourself")
    hooks.write_text(HOOK.format(flags="--fail-on-regression" if a.block else ""))
    hooks.chmod(0o755)
    print(f"installed {hooks}" + (" (blocks pushes on regressions)" if a.block else ""))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd")
    r = sub.add_parser("run", help="measure, record and compare")
    g = r.add_mutually_exclusive_group()
    g.add_argument("--headless", action="store_true", help="skip the app; binary size + VT parse only")
    g.add_argument("--full", action="store_true", help="longer settle/idle windows, 5 startup runs")
    r.add_argument("--label", help="free-form note stored with the run")
    r.add_argument("--no-build", action="store_true", help="use the existing release binary")
    r.add_argument("--busy", action="store_true", help="mark the run as not representative (kept out of baselines)")
    r.add_argument("--dry-run", action="store_true", help="don't record the run")
    r.add_argument("--baseline", type=int, default=10, metavar="N", help="compare with the last N runs (default 10)")
    r.add_argument("--fail-on-regression", action="store_true", help="exit 1 on a regression")
    c = sub.add_parser("compare", help="compare a recorded run with the runs before it")
    c.add_argument("id", nargs="?")
    c.add_argument("--baseline", type=int, default=10, metavar="N")
    c.add_argument("--fail-on-regression", action="store_true")
    h = sub.add_parser("history", help="list runs on this machine, or one metric over time")
    h.add_argument("metric", nargs="?", help=", ".join(METRICS))
    h.add_argument("-n", type=int, default=20)
    rp = sub.add_parser("report", help="write an HTML (default) or Markdown report and open it")
    rp.add_argument("--md", action="store_true", help="Markdown instead of HTML")
    rp.add_argument("-o", "--out", help="output path, or - for stdout (default target/perf-report.html|md)")
    rp.add_argument("--no-open", action="store_true", help="don't open the HTML report")
    rp.add_argument("--baseline", type=int, default=10, metavar="N")
    i = sub.add_parser("install-hook", help="run the headless suite in a git pre-push hook")
    i.add_argument("--block", action="store_true", help="fail the push on a regression")
    sub.add_parser("path", help="print the history file's path")
    a = p.parse_args()
    if a.cmd is None:
        p.print_help()
        return
    if a.cmd == "path":
        print(history_path())
        return
    {"run": cmd_run, "compare": cmd_compare, "report": cmd_report, "history": cmd_history, "install-hook": cmd_install_hook}[a.cmd](a)


if __name__ == "__main__":
    main()
