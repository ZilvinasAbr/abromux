#!/usr/bin/env bash
# Compare abromux with cmux: startup, memory, idle CPU and terminal throughput.
# Both apps are driven through their CLIs, which share command names.
#
# usage: scripts/bench.sh <abromux|cmux> <out-dir>
#   ABROMUX_BIN    release abromux binary (default target/release/abromux)
#   CMUX_APP    cmux.app to run; use a copy with its own bundle id so it starts
#               clean and doesn't touch your live cmux (see the README of the
#               results). Its socket must allow outside callers
#               (automation.socketControlMode = "allowAll" in cmux.json).
#   CMUX_SOCK   that copy's socket (default ~/.local/state/cmux/cmux-$UID.sock)
#   RUNS        startup runs (default 5)
set -euo pipefail
cd "$(dirname "$0")/.."
APP=$1
OUT=$2
RUNS=${RUNS:-5}
mkdir -p "$OUT"
DATA="$OUT/data"
LOG="$OUT/$APP.txt"
: > "$LOG"
say() { echo "$*" | tee -a "$LOG"; }
now() { python3 -c 'import time; print(time.time())'; }

case $APP in
  abromux)
    BIN=${ABROMUX_BIN:-$PWD/target/release/abromux}
    export ABROMUX_SOCKET_PATH=${ABROMUX_SOCKET_PATH:-/tmp/abromux-bench.sock}
    export ABROMUX_SESSION_PATH="$OUT/abromux-session.json"
    cli() { "$BIN" "$@"; }
    # Same window size as a fresh cmux (1000x700) and Ghostty's default
    # font size (13), so both grids match.
    export XDG_CONFIG_HOME="$OUT/xdg"
    mkdir -p "$XDG_CONFIG_HOME/ghostty"
    echo "font-size = 13" > "$XDG_CONFIG_HOME/ghostty/config"
    launch() {
      cat > "$ABROMUX_SESSION_PATH" <<JSON
{"version":1,"window":{"x":364,"y":200,"width":1000,"height":700},"active":0,"workspaces":[{"root":{"kind":"pane","tabs":[{"kind":"terminal","cwd":"$HOME"}]}}]}
JSON
      "$BIN" >/dev/null 2>&1 &
    }
    pid() { pgrep -f "^$BIN\$" | head -1; }
    ;;
  cmux)
    CMUX_APP=${CMUX_APP:?set CMUX_APP to a cmux.app copy}
    SOCK=${CMUX_SOCK:-$HOME/.local/state/cmux/cmux-$UID.sock}
    export CMUX_QUIET=1
    cli() { /Applications/cmux.app/Contents/Resources/bin/cmux --socket "$SOCK" "$@"; }
    launch() { "$CMUX_APP/Contents/MacOS/cmux" >/dev/null 2>&1 & }
    pid() { pgrep -f "^$CMUX_APP/Contents/MacOS/cmux\$" | head -1; }
    ;;
  *) echo "usage: $0 <abromux|cmux> <out-dir>"; exit 1 ;;
esac

quit_app() {
  local p; p=$(pid || true)
  [ -z "$p" ] && return
  kill -TERM "$p" 2>/dev/null || true
  for _ in $(seq 1 50); do kill -0 "$p" 2>/dev/null || return 0; sleep 0.1; done
  kill -KILL "$p" 2>/dev/null || true
}

footprint_mb() { footprint -p "$1" 2>/dev/null | sed -n 's/.*Footprint: \([0-9.]*\) \([KMG]B\).*/\1 \2/p' | awk '{ v=$1; if ($2=="GB") v*=1024; if ($2=="KB") v/=1024; print v }'; }
cpu_secs() { ps -o time= -p "$1" | awk -F'[:.]' '{ if (NF==3) print $1*60+$2+$3/100; else print $1*3600+$2*60+$3+$4/100 }'; }

# Wait until the focused terminal shows a shell prompt.
wait_prompt() {
  for _ in $(seq 1 300); do
    cli read-screen 2>/dev/null | grep -q '[%$#] *$' && return 0
    sleep 0.02
  done
  return 1
}

# ---- data --------------------------------------------------------------
if [ ! -f "$DATA/ascii.txt" ]; then
  mkdir -p "$DATA"
  python3 - "$DATA" <<'EOF'
import random, sys
d = sys.argv[1]
random.seed(1)
words = "the quick brown fox jumps over lazy dog terminal emulator throughput benchmark".split()
with open(f"{d}/ascii.txt", "w") as f:  # ~32 MB of plain lines
    for i in range(400_000):
        f.write(f"{i:07d} " + " ".join(random.choice(words) for _ in range(10)) + "\n")
with open(f"{d}/color.txt", "w") as f:  # ~24 MB, an SGR change every word
    for i in range(150_000):
        f.write("".join(f"\x1b[38;2;{random.randrange(256)};{random.randrange(256)};{random.randrange(256)}m{random.choice(words)} " for _ in range(8)) + "\x1b[0m\n")
cjk = "終端模擬器の性能を測定する😀🚀✅日本語中文한국어"
with open(f"{d}/unicode.txt", "w") as f:  # ~18 MB of wide chars and emoji
    for i in range(150_000):
        f.write("".join(random.choice(cjk) for _ in range(40)) + "\n")
EOF
fi

# ---- 1. startup --------------------------------------------------------
quit_app
say "# $APP"
for r in $(seq 1 "$RUNS"); do
  t0=$(now)
  launch
  until cli ping >/dev/null 2>&1; do sleep 0.01; done
  t1=$(now)
  wait_prompt || say "  (prompt not seen)"
  t2=$(now)
  say "startup run $r: socket ready $(python3 -c "print(round(($t1-$t0)*1000))") ms, prompt $(python3 -c "print(round(($t2-$t0)*1000))") ms"
  [ "$r" -lt "$RUNS" ] && { quit_app; sleep 1; }
done

# Keep the app in front: macOS stops drawing covered windows and App Naps
# their apps, which would skew everything below.
front() { cli focus-window --window window:1 >/dev/null 2>&1 || true; }

# ---- 2. idle memory and CPU ---------------------------------------------
front
sleep 15
P=$(pid)
say "idle footprint (1 terminal): $(footprint_mb "$P") MB"
c0=$(cpu_secs "$P"); sleep 30; c1=$(cpu_secs "$P")
say "idle CPU over 30 s: $(python3 -c "print(round(($c1-$c0)/30*100, 2))") %"

# ---- 3. per-terminal memory ---------------------------------------------
front
m0=$(footprint_mb "$P")
for _ in $(seq 1 10); do cli new-workspace >/dev/null; done
sleep 15
m1=$(footprint_mb "$P")
say "10 more terminals: $m0 -> $m1 MB ($(python3 -c "print(round(($m1-$m0)/10, 1))") MB each)"
c0=$(cpu_secs "$P"); sleep 30; c1=$(cpu_secs "$P")
say "idle CPU with 11 terminals over 30 s: $(python3 -c "print(round(($c1-$c0)/30*100, 2))") %"

# ---- 4. throughput -------------------------------------------------------
# Run in a fresh terminal. The shell times each cat up to the terminal's
# reply to a device-attributes query sent right after it: a terminal answers
# only once it has parsed everything before, so this counts processed bytes,
# not bytes sitting in a read buffer.
cli new-workspace >/dev/null
front
sleep 2
wait_prompt || true
RES="$OUT/$APP-throughput.txt"
rm -f "$RES"
cli send "stty size > $OUT/$APP-size.txt\n"
sleep 1
say "grid (rows cols): $(cat "$OUT/$APP-size.txt")"
[ "$(cat "$OUT/$APP-size.txt")" = "24 80" ] && say "WARNING: terminal never laid out (window not drawn?)"
c0=$(cpu_secs "$P")
cat > "$OUT/throughput.zsh" <<'ZSH'
zmodload zsh/datetime
data=$1 res=$2
old=$(stty -g)
stty -echo -icanon min 1
for i in 1 2 3; do
  for f in ascii color unicode; do
    s=$EPOCHREALTIME
    cat $data/$f.txt
    printf '\e[c'
    read -r -d c reply
    e=$EPOCHREALTIME
    print "$f $((e - s))" >> $res
  done
done
stty $old
print done >> $res
clear
ZSH
cmd="zsh $OUT/throughput.zsh $DATA $RES\n"
cli send "$cmd"
for _ in $(seq 1 1200); do grep -q '^done' "$RES" 2>/dev/null && break; sleep 0.5; done
c1=$(cpu_secs "$P")
for f in ascii color unicode; do
  mb=$(python3 -c "import os; print(round(os.path.getsize('$DATA/$f.txt')/1e6, 1))")
  python3 - "$RES" "$f" "$mb" <<'EOF' | tee -a "$LOG"
import sys, statistics
res, f, mb = sys.argv[1], sys.argv[2], float(sys.argv[3])
t = [float(l.split()[1]) for l in open(res) if l.startswith(f + " ")]
m = statistics.median(t)
print(f"throughput {f:<7} {mb:5.1f} MB: median {m:.2f} s ({mb/m:.1f} MB/s)  runs: " + " ".join(f"{x:.2f}" for x in t))
EOF
done
say "app CPU time during throughput: $(python3 -c "print(round($c1-$c0, 1))") s"
say "footprint after throughput: $(footprint_mb "$P") MB"
quit_app
