// The Claude tab's page. abromux sends events with window.__chat.recv([...])
// and gets the page's requests through window.ipc.postMessage.
(() => {
  "use strict";

  const $ = (id) => document.getElementById(id);
  const scroller = $("scroll");
  const thread = $("thread");
  const input = $("input");
  const sendBtn = $("send");
  const modeBtn = $("mode");
  const modelBtn = $("model");
  const menu = $("menu");
  const slash = $("slash");
  const working = $("working");
  const workingText = $("working-text");
  const toBottom = $("to-bottom");
  const attachments = $("attachments");

  const post = (msg) => {
    try { window.ipc.postMessage(JSON.stringify(msg)); } catch (_) {}
  };

  // Claude Code's permission modes. ⇧Tab cycles the first four, as in the terminal.
  const MODES = [
    { value: "default", name: "Ask permissions", short: "Ask", desc: "Ask before edits and commands", cycle: true },
    { value: "acceptEdits", name: "Accept edits", short: "Accept edits", desc: "Edit files without asking; ask before commands", cycle: true },
    { value: "plan", name: "Plan mode", short: "Plan", desc: "Read and plan; change nothing until you approve", cycle: true },
    { value: "auto", name: "Auto mode", short: "Auto", desc: "Claude checks each action and runs the safe ones", cycle: true, needsAuto: true },
    { value: "dontAsk", name: "Don't ask", short: "Don't ask", desc: "Run only what's already allowed; deny the rest" },
    { value: "bypassPermissions", name: "Bypass permissions", short: "Bypass", desc: "Run everything without asking. Only for sandboxes", danger: true },
  ];
  const EFFORT_NAMES = { low: "Low", medium: "Medium", high: "High", xhigh: "Extra high", max: "Max" };
  // Until Claude says which levels the model takes.
  const ALL_EFFORTS = ["low", "medium", "high", "xhigh", "max"];
  const FALLBACK_MODELS = [
    { value: "default", name: "Default", description: "Recommended model" },
    { value: "opus", name: "Opus", description: "For complex work" },
    { value: "sonnet", name: "Sonnet", description: "Fast and capable" },
    { value: "haiku", name: "Haiku", description: "Fastest" },
  ];

  const S = {
    items: new Map(),     // key -> element
    tools: new Map(),     // tool id -> {el, data}
    asks: new Map(),      // reqId -> element
    meta: {},
    commands: [],
    models: [],
    state: "idle",
    started: 0,
    images: [],
    slashSel: 0,
    stick: true,
    warmed: false,
    used: 0,          // context tokens in use, from the last reply
    popKind: null,    // what the popover over the composer shows: "slash" | "files"
    fileItems: [],
  };

  // ---- helpers ---------------------------------------------------------

  const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const h = (tag, cls, html) => {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (html != null) el.innerHTML = html;
    return el;
  };
  const basename = (p) => String(p || "").split("/").filter(Boolean).pop() || p || "";
  const shortPath = (p) => {
    const s = String(p || "");
    const cwd = S.meta.cwdAbs;
    if (cwd && s.startsWith(cwd + "/")) return s.slice(cwd.length + 1);
    return s.replace(/^\/Users\/[^/]+/, "~");
  };
  const firstLine = (s, n = 90) => {
    const l = String(s || "").split("\n").find((x) => x.trim()) || "";
    return l.length > n ? l.slice(0, n - 1) + "…" : l;
  };
  const lines = (s) => (s ? String(s).replace(/\n$/, "").split("\n").length : 0);
  const chevron = '<svg class="chev" viewBox="0 0 12 12"><path d="M4.5 2.5 8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';


  // What a tool call says in its one line: [icon, verb, target].
  function describe(name, input) {
    const i = input || {};
    switch (name) {
      case "Bash": return ["terminal", "Ran", firstLine(i.command, 200) || i.description || ""];
      case "BashOutput": return ["terminal", "Read output", i.bash_id || ""];
      case "KillShell": case "KillBash": return ["terminal", "Stopped shell", i.shell_id || ""];
      case "Read": return ["file", "Read", shortPath(i.file_path)];
      case "Write": return ["edit", "Wrote", shortPath(i.file_path)];
      case "Edit": return ["edit", "Edited", shortPath(i.file_path)];
      case "MultiEdit": return ["edit", "Edited", shortPath(i.file_path)];
      case "NotebookEdit": return ["edit", "Edited notebook", shortPath(i.notebook_path)];
      case "Grep": return ["search", "Searched", i.pattern ? `"${i.pattern}"${i.path ? " in " + shortPath(i.path) : ""}` : ""];
      case "Glob": return ["search", "Found files", i.pattern || ""];
      case "LS": return ["search", "Listed", shortPath(i.path)];
      case "WebFetch": return ["globe", "Fetched", i.url || ""];
      case "WebSearch": return ["globe", "Searched the web", i.query || ""];
      case "TodoWrite": return ["list", "Updated todos", ""];
      case "Task": case "Agent": return ["agent", i.subagent_type ? `Agent · ${i.subagent_type}` : "Agent", i.description || ""];
      case "ExitPlanMode": return ["plan", "Plan", ""];
      case "AskUserQuestion": return ["agent", "Asked", (i.questions && i.questions[0] && i.questions[0].question) || ""];
      case "Skill": return ["tool", "Skill", i.skill || ""];
      default: {
        const m = /^mcp__(.+?)__(.+)$/.exec(name || "");
        if (m) return ["tool", m[2].replace(/_/g, " "), m[1].replace(/^plugin_[^_]+_/, "")];
        return ["tool", name || "Tool", ""];
      }
    }
  }

  // ---- diffs ------------------------------------------------------------

  function diffLines(a, b) {
    const A = String(a ?? "").split("\n");
    const B = String(b ?? "").split("\n");
    if (A.length * B.length > 250000) {
      return [...A.map((l) => ["-", l]), ...B.map((l) => ["+", l])];
    }
    // Longest common subsequence, table from the end.
    const n = A.length, m = B.length;
    const T = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let i = n - 1; i >= 0; i--)
      for (let j = m - 1; j >= 0; j--)
        T[i][j] = A[i] === B[j] ? T[i + 1][j + 1] + 1 : Math.max(T[i + 1][j], T[i][j + 1]);
    const out = [];
    let i = 0, j = 0;
    while (i < n && j < m) {
      if (A[i] === B[j]) { out.push([" ", A[i]]); i++; j++; }
      else if (T[i + 1][j] >= T[i][j + 1]) out.push(["-", A[i++]]);
      else out.push(["+", B[j++]]);
    }
    while (i < n) out.push(["-", A[i++]]);
    while (j < m) out.push(["+", B[j++]]);
    return out;
  }

  // ---- syntax highlighting (highlight.js, /__chat/hl.js) ------------------

  const hl = window.hljs || null;
  const EXT_LANG = {
    rs: "rust", ts: "typescript", tsx: "typescript", mts: "typescript", cts: "typescript",
    js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
    py: "python", go: "go", php: "php", rb: "ruby", java: "java", kt: "kotlin", kts: "kotlin",
    swift: "swift", c: "c", h: "c", cc: "cpp", cpp: "cpp", cxx: "cpp", hpp: "cpp", hh: "cpp",
    cs: "csharp", css: "css", scss: "scss", less: "less", html: "xml", htm: "xml", xml: "xml",
    svg: "xml", vue: "xml", json: "json", jsonc: "json", yaml: "yaml", yml: "yaml", toml: "ini",
    ini: "ini", cfg: "ini", md: "markdown", markdown: "markdown", sh: "bash", bash: "bash",
    zsh: "bash", fish: "bash", sql: "sql", lua: "lua", pl: "perl", pm: "perl", r: "r",
    m: "objectivec", mm: "objectivec", graphql: "graphql", gql: "graphql", mk: "makefile",
    diff: "diff", patch: "diff",
  };
  const LANG_ALIAS = { sh: "bash", shell: "bash", zsh: "bash", console: "bash", js: "javascript", ts: "typescript",
    py: "python", rb: "ruby", rs: "rust", yml: "yaml", toml: "ini", html: "xml", jsonc: "json", patch: "diff",
    "c++": "cpp", "objective-c": "objectivec", golang: "go", kt: "kotlin", cs: "csharp" };

  // The language for a code fence's name or a file's path, if highlight.js has it.
  function langFor(nameOrPath) {
    if (!hl || !nameOrPath) return null;
    let n = String(nameOrPath).toLowerCase();
    const base = n.split("/").pop();
    if (base === "makefile" || base === "gnumakefile") return "makefile";
    if (base === "dockerfile") return "bash";
    if (n.includes(".") || n.includes("/")) n = EXT_LANG[base.split(".").pop()] || "";
    n = LANG_ALIAS[n] || n;
    return n && hl.getLanguage(n) ? n : null;
  }
  // `text` as highlighted HTML (escaped if there's no language).
  function paint(text, lang) {
    if (!lang) return esc(text);
    try { return hl.highlight(text, { language: lang, ignoreIllegals: true }).value; } catch (_) { return esc(text); }
  }
  // Highlight all of `lines` at once (so a string or comment spanning
  // lines stays colored), then split the HTML back into lines, closing the
  // spans open at each line's end and reopening them on the next.
  function paintLines(lines, lang) {
    if (!lang) return lines.map(esc);
    const html = paint(lines.join("\n"), lang);
    const out = [];
    let open = [], cur = "";
    const re = /<span[^>]*>|<\/span>|\n|[^<\n]+|</g;
    let m;
    while ((m = re.exec(html))) {
      const t = m[0];
      if (t === "\n") {
        out.push(cur + "</span>".repeat(open.length));
        cur = open.join("");
      } else {
        if (t.startsWith("<span")) open.push(t);
        else if (t === "</span>") open.pop();
        cur += t;
      }
    }
    out.push(cur);
    return out;
  }

  // A unified diff (a ```diff block): rows like the edit cards', the code
  // colored in the language of the file it changes.
  function renderUnified(text) {
    const d = h("div", "diff unified");
    let lang = null, file = "";
    const rows = String(text).replace(/\n$/, "").split("\n");
    const content = [];
    for (const l of rows) {
      const m = /^(?:\+\+\+ (?:b\/)?|--- (?:a\/)?|diff --git a\/\S+ b\/)(\S+)/.exec(l);
      if (m && m[1] !== "/dev/null") { file = m[1]; lang = langFor(m[1]); }
      let kind = " ";
      if (/^(diff |index |--- |\+\+\+ |new file|deleted file|similarity|rename )/.test(l)) kind = "meta";
      else if (l.startsWith("@@")) kind = "hunk";
      else if (l.startsWith("+")) kind = "+";
      else if (l.startsWith("-")) kind = "-";
      content.push([kind, l, lang]);
    }
    // Color the new side (added and context lines) and the old side
    // (removed lines) each as one stream per file, so multi-line strings
    // and comments hold.
    const painted = new Array(content.length);
    let start = 0;
    const flush = (end) => {
      const L = content[start] ? content[start][2] : null;
      for (const keep of [(k) => k === "+" || k === " ", (k) => k === "-"]) {
        const idx = [];
        for (let i = start; i < end; i++) if (keep(content[i][0])) idx.push(i);
        const out = paintLines(idx.map((i) => content[i][1].slice(1)), L);
        idx.forEach((i, k) => (painted[i] = out[k]));
      }
      start = end;
    };
    content.forEach((c, i) => { if (c[0] === "meta" && i > start && content[i - 1][0] !== "meta") flush(i); });
    flush(content.length);
    content.forEach(([kind, l], i) => {
      if (kind === "meta" || kind === "hunk") {
        d.append(h("div", "diff-line " + kind, `<span class="sign"></span><span>${esc(l)}</span>`));
        return;
      }
      const cls = kind === "+" ? "add" : kind === "-" ? "del" : "";
      d.append(h("div", "diff-line " + cls, `<span class="sign">${kind === " " ? "" : kind}</span><span>${painted[i] ?? esc(l.slice(1))}</span>`));
    });
    return { el: d, file };
  }

  // A file read back (`cat -n` style: number, tab or →, line) as
  // [number, line] pairs; null if it isn't one.
  function readLines(text) {
    const out = [];
    for (const l of String(text).replace(/\n$/, "").split("\n")) {
      const m = /^\s*(\d+)[\t→](.*)$/.exec(l);
      if (!m) return out.length > 3 ? out : null;
      out.push([m[1], m[2]]);
    }
    return out.length ? out : null;
  }

  function renderDiff(pairs, file) {
    const box = h("div");
    if (file) box.append(h("div", "diff-file", esc(file)));
    const d = h("div", "diff");
    let adds = 0, dels = 0;
    const lang = langFor(file || renderDiff.path);
    for (const [oldS, newS] of pairs) {
      const ops = diffLines(oldS, newS);
      // Each side colored as a whole, so strings and comments that span
      // lines come out right.
      const oldP = paintLines(String(oldS ?? "").split("\n"), lang);
      const newP = paintLines(String(newS ?? "").split("\n"), lang);
      let oi = 0, ni = 0;
      const html = ops.map((op) => (op[0] === "-" ? oldP[oi++] : op[0] === "+" ? newP[ni++] : (oi++, newP[ni++])));
      // Keep two lines of context around changes.
      const keep = ops.map(() => false);
      ops.forEach((op, k) => {
        if (op[0] !== " ") for (let x = Math.max(0, k - 2); x <= Math.min(ops.length - 1, k + 2); x++) keep[x] = true;
      });
      let gap = false;
      ops.forEach((op, k) => {
        if (!keep[k]) { gap = true; return; }
        if (gap) { d.append(h("div", "diff-gap", "⋯")); gap = false; }
        const cls = op[0] === "+" ? "add" : op[0] === "-" ? "del" : "";
        if (op[0] === "+") adds++;
        if (op[0] === "-") dels++;
        const row = h("div", "diff-line " + cls);
        row.append(h("span", "sign", op[0] === " " ? "" : op[0]));
        row.append(h("span", "", html[k] || " "));
        d.append(row);
      });
    }
    box.append(d);
    return { el: box, adds, dels };
  }

  // ---- markdown post-processing ----------------------------------------

  function enhance(root) {
    root.querySelectorAll("pre > code").forEach((code) => {
      const pre = code.parentElement;
      if (pre.parentElement.classList.contains("codeblock")) return;
      const lang = ((code.className || "").match(/language-([\w+#-]+)/) || [])[1] || "";
      const raw = code.textContent;
      const box = h("div", "codeblock");
      const head = h("div", "codeblock-head");
      let label = lang || "code";
      if (/^(diff|patch)$/i.test(lang) || (!lang && /^(--- |diff --git )/m.test(raw) && /^@@ /m.test(raw))) {
        const u = renderUnified(raw);
        if (u.file) label = u.file;
        pre.classList.add("has-diff");
        pre.replaceChildren(u.el);
      } else {
        const L = langFor(lang);
        if (L) {
          code.innerHTML = paint(raw, L);
          code.classList.add("hljs");
        }
      }
      head.append(h("span", "", esc(label)));
      const copy = h("button", "copy", "Copy");
      copy.addEventListener("click", () => {
        copyText(raw);
        copy.textContent = "Copied";
        setTimeout(() => (copy.textContent = "Copy"), 1400);
      });
      head.append(copy);
      pre.replaceWith(box);
      box.append(head, pre);
    });
  }

  function copyText(text) {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.append(ta);
    ta.select();
    try { document.execCommand("copy"); } catch (_) {}
    ta.remove();
  }

  // ---- scrolling --------------------------------------------------------

  const nearBottom = () => scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
  function follow() {
    if (S.stick) scroller.scrollTop = scroller.scrollHeight;
  }
  scroller.addEventListener("scroll", () => {
    S.stick = nearBottom();
    toBottom.hidden = S.stick || thread.childElementCount === 0;
  });
  toBottom.addEventListener("click", () => {
    S.stick = true;
    scroller.scrollTo({ top: scroller.scrollHeight, behavior: "smooth" });
  });

  // ---- rendering events -------------------------------------------------

  function place(key, el) {
    const old = key && S.items.get(key);
    if (old) old.replaceWith(el);
    else thread.append(el);
    if (key) S.items.set(key, el);
    $("empty").hidden = true;
  }

  const ICON_COPY = '<svg viewBox="0 0 16 16"><rect x="5.25" y="5.25" width="8" height="8" rx="1.75" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M10.75 5.25V4a1.25 1.25 0 0 0-1.25-1.25H4A1.25 1.25 0 0 0 2.75 4v5.5A1.25 1.25 0 0 0 4 10.75h1.25" fill="none" stroke="currentColor" stroke-width="1.3"/></svg>';
  const ICON_REWIND = '<svg viewBox="0 0 16 16"><path d="M3 8a5 5 0 1 0 1.5-3.55M3 3v2.75h2.75" fill="none" stroke="currentColor" stroke-width="1.35" stroke-linecap="round" stroke-linejoin="round"/></svg>';

  function copyButton(get, label = "Copy") {
    const b = h("button", "act", ICON_COPY);
    b.title = label;
    b.addEventListener("click", () => {
      copyText(get());
      b.classList.add("done");
      setTimeout(() => b.classList.remove("done"), 1200);
    });
    return b;
  }

  function onUser(e) {
    if (!S.turnClosed) closeTurn();
    S.turnTools = [];
    S.turnClosed = false;
    const el = h("div", "item user");
    const b = h("div", "bubble");
    if (e.images) b.append(h("span", "img-count", `${e.images} image${e.images > 1 ? "s" : ""}`));
    b.append(document.createTextNode(e.text || ""));
    const acts = h("div", "acts");
    acts.append(copyButton(() => e.text || ""));
    if (e.uuid) {
      const rw = h("button", "act", ICON_REWIND);
      rw.title = "Rewind to here: edit this message and go on from before it";
      rw.addEventListener("click", () => askRewind(e, el));
      acts.append(rw);
    }
    const col = h("div", "user-col");
    col.append(b, acts);
    el.append(col);
    el.dataset.uuid = e.uuid || "";
    place("u:" + e.key, el);
    S.stick = true;
    updateTitle();
  }

  function onText(e) {
    let el = S.items.get("t:" + e.key);
    if (!el) {
      el = h("div", "item md");
      place("t:" + e.key, el);
    }
    el.innerHTML = e.html || "";
    el.classList.toggle("live", !!e.live);
    enhance(el);
    if (!e.live && e.raw) {
      const acts = h("div", "acts md-acts");
      acts.append(copyButton(() => e.raw, "Copy reply"));
      el.append(acts);
    }
  }

  function onThinking(e) {
    let el = S.items.get("th:" + e.key);
    if (!el) {
      el = h("div", "item thinking");
      const btn = h("button", "step", '<span class="step-verb"></span>' + chevron);
      btn.addEventListener("click", () => el.classList.toggle("open"));
      el.append(btn, h("div", "thinking-body"));
      place("th:" + e.key, el);
      el._start = Date.now();
    }
    const label = el.querySelector(".step-verb");
    el.classList.toggle("live", !!e.live);
    if (e.live) label.textContent = "Thinking…";
    else {
      const secs = el._start && e.live === undefined && el._wasLive ? Math.max(1, Math.round((Date.now() - el._start) / 1000)) : 0;
      label.textContent = secs ? `Thought for ${secs}s` : "Thought process";
    }
    if (e.live) el._wasLive = true;
    const body = el.querySelector(".thinking-body");
    body.textContent = e.text || "";
    if (!e.text && !e.live) el.querySelector(".step").classList.add("static");
  }

  function onTool(e) {
    const prev = S.tools.get(e.id);
    const data = Object.assign({}, prev && prev.data, e);
    if (e.input == null && prev && prev.data.input) data.input = prev.data.input;
    let el = prev && prev.el;
    if (!el) {
      el = h("div", "item tool pending");
      place("tool:" + e.id, el);
      (S.turnTools || (S.turnTools = [])).push(e.id);
    }
    S.tools.set(e.id, { el, data });
    drawTool(el, data);
  }

  function drawTool(el, d) {
    const open = el.classList.contains("open");
    const [, verb, target] = describe(d.name, d.input);
    el.innerHTML = "";
    const head = h("button", "step");
    head.append(h("span", "step-verb", esc(verb)));
    if (target) head.append(h("span", "step-target", esc(target)));
    const liveSlot = h("span", "step-live");
    head.append(liveSlot);
    const meta = h("span", "stat");
    const body = h("div", "tool-body");
    const i = d.input || {};

    // What went in.
    renderDiff.path = i.file_path || i.notebook_path || "";
    if (d.name === "Edit" && i.old_string != null) {
      const r = renderDiff([[i.old_string, i.new_string]]);
      body.append(r.el);
      meta.innerHTML = `<span class="add">+${r.adds}</span><span class="del">−${r.dels}</span>`;
      d.adds = r.adds; d.dels = r.dels;
    } else if (d.name === "MultiEdit" && Array.isArray(i.edits)) {
      const r = renderDiff(i.edits.map((x) => [x.old_string, x.new_string]));
      body.append(r.el);
      meta.innerHTML = `<span class="add">+${r.adds}</span><span class="del">−${r.dels}</span>`;
      d.adds = r.adds; d.dels = r.dels;
    } else if (d.name === "Write" && i.content != null) {
      const r = renderDiff([["", i.content]]);
      body.append(r.el);
      meta.innerHTML = `<span class="add">+${lines(i.content)}</span>`;
      d.adds = lines(i.content); d.dels = 0;
    } else if (d.name === "TodoWrite" && Array.isArray(i.todos)) {
      const ul = h("ul", "todos");
      for (const t of i.todos) {
        const li = h("li", t.status);
        li.append(h("span", "box"), h("span", "", esc(t.status === "in_progress" && t.activeForm ? t.activeForm : t.content)));
        ul.append(li);
      }
      body.append(ul);
      const doneN = i.todos.filter((t) => t.status === "completed").length;
      meta.textContent = `${doneN}/${i.todos.length}`;
      el.classList.add("open");
    } else if (d.name === "Bash" && i.command) {
      body.append(h("div", "tool-section-label", "Command"));
      body.append(h("pre", "tool-pre", paint(i.command, langFor("bash"))));
    } else if (d.name === "Task" || d.name === "Agent") {
      if (i.prompt) {
        body.append(h("div", "tool-section-label", "Prompt"));
        body.append(h("pre", "tool-pre", esc(i.prompt)));
      }
      const subs = [...(d.subs || new Map()).values()];
      if (subs.length) {
        const steps = subs.filter((x) => x.name);
        meta.textContent = `${steps.length} step${steps.length === 1 ? "" : "s"}`;
        body.append(h("div", "tool-section-label", "Steps"));
        const ul = h("ul", "steps");
        for (const x of subs) {
          if (x.name) {
            const [, v, t] = describe(x.name, x.input);
            const li = h("li", x.error ? "failed" : x.done ? "" : "pending", `<span class="v">${esc(v)}</span> ${esc(t)}`);
            ul.append(li);
          } else if (x.text) {
            ul.append(h("li", "say", esc(x.text)));
          }
        }
        body.append(ul);
        // While it works, its latest step shows in the card's line.
        const last = subs[subs.length - 1];
        if (!d.result && last) {
          const [, v, t] = last.name ? describe(last.name, last.input) : ["", "", last.text];
          d.stepNow = (v ? v + " " : "") + (t || "");
        }
      }
    } else if (d.input && Object.keys(d.input).length && !["Read", "Glob", "Grep", "LS", "WebSearch", "WebFetch"].includes(d.name)) {
      body.append(h("div", "tool-section-label", "Input"));
      body.append(h("pre", "tool-pre", esc(JSON.stringify(d.input, null, 2))));
    }

    // What came out.
    if (d.result) {
      const r = d.result;
      if (d.name === "Read" && !r.error) meta.textContent = `${lines(r.text)} lines`;
      else if ((d.name === "Grep" || d.name === "Glob") && !r.error && r.text) meta.textContent = `${lines(r.text)} results`;
      const hide = !r.error && ["Edit", "MultiEdit", "Write", "TodoWrite"].includes(d.name);
      if (!hide && (r.text || r.images)) {
        if (body.childElementCount) body.append(h("div", "tool-sep"));
        body.append(h("div", "tool-section-label", r.error ? "Error" : "Output"));
        const text = r.text || `${r.images} image${r.images > 1 ? "s" : ""}`;
        const numbered = d.name === "Read" && !r.error && readLines(text);
        if (numbered) {
          const L = langFor(i.file_path);
          const code = paintLines(numbered.map((x) => x[1]), L);
          const pre = h("div", "tool-pre code-lines");
          numbered.forEach(([n], k) => pre.append(h("div", "code-line", `<span class="ln">${n}</span><span>${code[k] || " "}</span>`)));
          body.append(pre);
        } else {
          body.append(h("pre", "tool-pre" + (r.error ? " err" : ""), esc(text)));
        }
      } else if (r.error && r.text) {
        body.append(h("pre", "tool-pre err", esc(r.text)));
      }
    }
    if (d.stepNow && !d.result) liveSlot.textContent = "· " + d.stepNow;
    if (meta.childNodes.length) head.append(meta);
    if (body.childElementCount) head.insertAdjacentHTML("beforeend", chevron);
    else head.classList.add("static");
    head.addEventListener("click", () => body.childElementCount && el.classList.toggle("open"));
    el.append(head);
    if (body.childElementCount) el.append(body);
    if (open) el.classList.add("open");
    el.classList.toggle("pending", !d.result);
    el.classList.toggle("done", !!d.result && !d.result.error);
    el.classList.toggle("failed", !!d.result && !!d.result.error);
  }

  function onToolResult(e) {
    let t = S.tools.get(e.id);
    if (!t) {
      onTool({ id: e.id, name: "Tool", input: null });
      t = S.tools.get(e.id);
    }
    t.data.result = { text: e.text, error: e.error, images: e.images };
    drawTool(t.el, t.data);
  }

  function onSub(e, done) {
    let t = S.tools.get(e.parent);
    if (!t) return;
    const subs = t.data.subs || (t.data.subs = new Map());
    const x = subs.get(e.id) || {};
    if (done) Object.assign(x, { done: true, error: !!e.error });
    else Object.assign(x, { name: e.name, input: e.input, text: e.text });
    subs.set(e.id, x);
    drawTool(t.el, t.data);
  }

  // ---- what a turn changed ---------------------------------------------

  // At a turn's end: the files it edited, with their +/− and a way back
  // to each edit.
  function closeTurn() {
    S.turnClosed = true;
    // The turn's last reply gets the copy button.
    for (let n = thread.lastElementChild; n && !n.classList.contains("user"); n = n.previousElementSibling) {
      if (n.classList.contains("md")) { n.classList.add("last"); break; }
    }
    const files = new Map();
    for (const id of S.turnTools || []) {
      const t = S.tools.get(id);
      const d = t && t.data;
      if (!d || !d.result || d.result.error || d.adds == null) continue;
      const path = d.input && (d.input.file_path || d.input.notebook_path);
      if (!path) continue;
      const f = files.get(path) || { adds: 0, dels: 0, el: null };
      f.adds += d.adds; f.dels += d.dels; f.el = t.el;
      files.set(path, f);
    }
    if (!files.size) return;
    let A = 0, D = 0;
    files.forEach((f) => { A += f.adds; D += f.dels; });
    const box = h("div", "item changes");
    const head = h("button", "step");
    head.innerHTML = `<span class="step-verb">Changed ${files.size} file${files.size > 1 ? "s" : ""}</span><span class="stat"><span class="add">+${A}</span><span class="del">−${D}</span></span>${chevron}`;
    const list = h("div", "changes-list");
    files.forEach((f, path) => {
      const row = h("button", "change-row", `<span class="file">${esc(shortPath(path))}</span><span class="stat"><span class="add">+${f.adds}</span><span class="del">−${f.dels}</span></span>`);
      row.addEventListener("click", () => {
        f.el.classList.add("open");
        f.el.scrollIntoView({ block: "center", behavior: "smooth" });
        f.el.classList.add("flash");
        setTimeout(() => f.el.classList.remove("flash"), 1200);
      });
      list.append(row);
    });
    head.addEventListener("click", () => box.classList.toggle("open"));
    box.append(head, list);
    if (files.size <= 4) box.classList.add("open");
    thread.append(box);
  }

  // ---- rewind ------------------------------------------------------------

  function askRewind(e, userEl) {
    document.querySelectorAll(".rewind").forEach((x) => x.remove());
    const card = h("div", "item ask rewind");
    card.append(h("div", "ask-head", "<span>Rewind to before this message?</span>"));
    card.append(h("div", "ask-desc", "The conversation goes back to here and your message returns to the composer to edit. Claude forgets what came after."));
    const files = h("div", "ask-desc rewind-files", "Checking which files changed since…");
    card.append(files);
    const actions = h("div", "ask-actions");
    const cancel = h("button", "btn", "Cancel");
    const conv = h("button", "btn", "Rewind conversation");
    const both = h("button", "btn primary", "Rewind conversation and code");
    both.hidden = true;
    cancel.addEventListener("click", () => card.remove());
    conv.addEventListener("click", () => { card.remove(); post({ chat: "rewind", uuid: e.uuid, code: false }); });
    both.addEventListener("click", () => { card.remove(); post({ chat: "rewind", uuid: e.uuid, code: true }); });
    actions.append(cancel, conv, both);
    card.append(actions);
    card.dataset.uuid = e.uuid;
    userEl.after(card);
    card.scrollIntoView({ block: "nearest", behavior: "smooth" });
    post({ chat: "rewind_preview", uuid: e.uuid });
  }

  function onRewindPreview(e) {
    const card = document.querySelector(`.rewind[data-uuid="${CSS.escape(e.uuid)}"]`);
    if (!card) return;
    const files = card.querySelector(".rewind-files");
    const n = (e.files || []).length;
    if (e.canRewind && n) {
      const names = e.files.slice(0, 4).map((f) => esc(basename(f))).join(", ") + (n > 4 ? `, and ${n - 4} more` : "");
      files.innerHTML = `Code: ${n} file${n > 1 ? "s" : ""} changed since then (${names}), <span class="add">+${e.insertions || 0}</span> <span class="del">−${e.deletions || 0}</span>. You can restore ${n > 1 ? "them" : "it"} too.`;
      card.querySelector(".btn.primary").hidden = false;
      card.querySelectorAll(".btn")[1].classList.remove("primary");
    } else {
      files.textContent = n === 0 && !e.error ? "No files changed since then." : "Files can't be restored from here; only the conversation rewinds.";
      card.querySelectorAll(".btn")[1].classList.add("primary");
    }
  }

  // ---- context ------------------------------------------------------------

  function drawContext() {
    const ctx = $("ctx");
    const win = S.meta.contextWindow || (/\[1m\]/.test(S.meta.model || "") ? 1000000 : 200000);
    if (!S.used) { ctx.hidden = true; return; }
    const pct = Math.min(100, Math.round((S.used / win) * 100));
    const k = (n) => (n >= 1000 ? `${Math.round(n / 1000)}k` : String(n));
    ctx.hidden = false;
    ctx.title = `Context: ${k(S.used)} of ${k(win)} tokens used (${pct}%). /compact frees it up.`;
    const r = 6, c = 2 * Math.PI * r;
    ctx.innerHTML = `<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="${r}" fill="none" stroke="currentColor" stroke-opacity="0.25" stroke-width="2"/><circle cx="8" cy="8" r="${r}" fill="none" stroke="currentColor" stroke-width="2" stroke-dasharray="${(c * pct) / 100} ${c}" transform="rotate(-90 8 8)" stroke-linecap="round"/></svg><span>${pct}%</span>`;
    ctx.classList.toggle("high", pct >= 80);
  }

  // ---- permission prompts ----------------------------------------------

  function onPermission(e) {
    const el = h("div", "item ask");
    el.dataset.req = e.reqId;
    const tool = e.tool;
    const i = e.input || {};
    const head = h("div", "ask-head");
    head.append(h("span", "spark"));

    if (tool === "AskUserQuestion" && Array.isArray(i.questions)) {
      head.append(h("span", "", "Claude has a question"));
      el.append(head);
      const answers = {};
      const qs = i.questions;
      qs.forEach((q) => {
        const box = h("div", "question");
        box.append(h("h4", "", esc(q.question)));
        const opts = h("div", "options");
        (q.options || []).forEach((o) => {
          const b = h("button", "option", `<b>${esc(o.label)}</b>${o.description ? `<span>${esc(o.description)}</span>` : ""}`);
          b.addEventListener("click", () => {
            if (q.multiSelect) {
              b.classList.toggle("picked");
              const picked = [...opts.querySelectorAll(".picked b")].map((x) => x.textContent);
              answers[q.question] = picked.join(", ");
            } else {
              opts.querySelectorAll(".option").forEach((x) => x.classList.remove("picked"));
              b.classList.add("picked");
              answers[q.question] = o.label;
            }
          });
          opts.append(b);
        });
        box.append(opts);
        el.append(box);
      });
      const actions = h("div", "ask-actions");
      const other = h("input");
      other.placeholder = "Or type your own answer…";
      const submit = h("button", "btn primary", "Answer");
      const skip = h("button", "btn", "Skip");
      submit.addEventListener("click", () => {
        if (other.value.trim()) qs.forEach((q) => { if (!answers[q.question]) answers[q.question] = other.value.trim(); });
        if (qs.some((q) => !answers[q.question])) { other.focus(); return; }
        answer(e.reqId, true, { updatedInput: Object.assign({}, i, { answers }) }, "Answered");
      });
      skip.addEventListener("click", () => answer(e.reqId, false, { message: "The user skipped the question." }, "Skipped"));
      actions.append(other, skip, submit);
      el.append(actions);
    } else if (tool === "ExitPlanMode") {
      head.append(h("span", "", "Ready to code?"));
      el.append(head);
      el.append(h("div", "ask-desc", "Here is Claude's plan. Approve it to start, or tell Claude what to change."));
      const plan = h("div", "ask-detail");
      const md = h("div", "md plan", e.planHtml || esc(i.plan || ""));
      enhance(md);
      plan.append(md);
      el.append(plan);
      const actions = h("div", "ask-actions");
      const fb = h("input");
      fb.placeholder = "Tell Claude what to change…";
      const keep = h("button", "btn", "Keep planning");
      const auto = h("button", "btn", "Approve, auto-accept edits");
      const go = h("button", "btn primary", "Approve");
      keep.addEventListener("click", () => answer(e.reqId, false, { message: fb.value.trim() || "The user wants to keep planning." }, "Kept planning"));
      auto.addEventListener("click", () => { setMode("acceptEdits"); answer(e.reqId, true, {}, "Plan approved"); });
      go.addEventListener("click", () => { setMode("default"); answer(e.reqId, true, {}, "Plan approved"); });
      fb.addEventListener("keydown", (ev) => { if (ev.key === "Enter" && fb.value.trim()) keep.click(); });
      actions.append(fb, keep, auto, go);
      el.append(actions);
    } else {
      const [, verb, target] = describe(tool, i);
      const title = {
        Bash: "Run this command?",
        Edit: "Make this edit?",
        MultiEdit: "Make these edits?",
        Write: `Write ${basename(i.file_path)}?`,
        WebFetch: "Fetch this page?",
        WebSearch: "Search the web?",
      }[tool] || `Allow ${verb}${target ? "" : ""}?`;
      head.append(h("span", "", esc(title)));
      el.append(head);
      if (e.description && tool !== "Bash") el.append(h("div", "ask-desc", esc(e.description)));
      else if (tool === "Bash" && i.description) el.append(h("div", "ask-desc", esc(i.description)));
      const detail = h("div", "ask-detail");
      if (tool === "Bash") detail.append(h("pre", "tool-pre", esc(i.command)));
      else if (tool === "Edit") detail.append(renderDiff([[i.old_string, i.new_string]], shortPath(i.file_path)).el);
      else if (tool === "MultiEdit") detail.append(renderDiff((i.edits || []).map((x) => [x.old_string, x.new_string]), shortPath(i.file_path)).el);
      else if (tool === "Write") detail.append(renderDiff([["", i.content]], shortPath(i.file_path)).el);
      else detail.append(h("pre", "tool-pre", esc(target || JSON.stringify(i, null, 2))));
      el.append(detail);
      const actions = h("div", "ask-actions");
      const deny = h("button", "btn", "Deny<kbd>esc</kbd>");
      const always = h("button", "btn", "Always allow");
      const allow = h("button", "btn primary", "Allow<kbd>↩</kbd>");
      deny.addEventListener("click", () => answer(e.reqId, false, {}, "Denied"));
      always.addEventListener("click", () => answer(e.reqId, true, { always: true }, "Always allowed"));
      allow.addEventListener("click", () => answer(e.reqId, true, {}, "Allowed"));
      actions.append(deny);
      if (e.canAlways) actions.append(always);
      actions.append(allow);
      el.append(actions);
    }
    const anchor = e.toolUseId && S.tools.get(e.toolUseId);
    if (anchor) {
      anchor.el.after(el);
      S.items.set("ask:" + e.reqId, el);
    } else place("ask:" + e.reqId, el);
    S.asks.set(e.reqId, el);
    S.stick = true;
    follow();
  }

  function answer(reqId, allow, extra, label) {
    post(Object.assign({ chat: "permission", reqId, allow }, extra));
    resolveAsk(reqId, label);
  }

  function resolveAsk(reqId, label) {
    const el = S.asks.get(reqId);
    if (!el) return;
    S.asks.delete(reqId);
    el.classList.add("resolved");
    const head = el.querySelector(".ask-head span:last-child");
    if (head && label) head.textContent = label;
    input.focus();
  }

  // ---- status ------------------------------------------------------------

  function onMeta(m) {
    S.meta = Object.assign(S.meta, m);
    if (m.cwdFull) S.meta.cwdAbs = m.cwdFull;
    if (m.dark !== undefined) document.documentElement.dataset.theme = m.dark ? "dark" : "light";
    const cwd = S.meta.cwd || "";
    // A long folder shows by its last two parts; hover for all of it.
    const parts = cwd.split("/");
    $("empty-cwd").textContent = cwd.length > 40 && parts.length > 3 ? "…/" + parts.slice(-2).join("/") : cwd;
    $("empty-cwd").title = S.meta.cwdFull || cwd;
    setState(m.state || S.state);
    drawMode();
    drawModel();
    drawContext();
  }

  function setState(state) {
    if (state === S.state) return;
    if (state === "running" && (S.state !== "waiting" || !S.started)) S.started = Date.now();
    S.state = state;
    drawWorking();
    drawSend();
  }

  function drawWorking() {
    const busy = S.state === "running";
    working.hidden = !busy;
    tickWorking();
  }

  const VERBS = ["Working", "Thinking", "Pondering", "Crafting", "Brewing", "Tinkering", "Noodling", "Percolating"];
  let verbIdx = 0;
  setInterval(() => { verbIdx = (verbIdx + 1) % VERBS.length; tickWorking(); }, 4000);
  setInterval(tickWorking, 1000);
  function tickWorking() {
    if (working.hidden) return;
    const s = Math.max(0, Math.round((Date.now() - S.started) / 1000));
    const live = thread.querySelector(".md.live");
    workingText.textContent = `${live ? "Writing" : VERBS[verbIdx]}… ${s}s`;
  }

  function drawSend() {
    const busy = S.state === "running" || S.state === "waiting";
    const has = input.value.trim() || S.images.length;
    sendBtn.classList.toggle("stop", busy && !has);
    sendBtn.disabled = !busy && !has;
    sendBtn.title = busy && !has ? "Stop (esc)" : "Send (↩)";
  }

  function drawMode() {
    const mode = S.meta.mode || "default";
    const m = MODES.find((x) => x.value === mode) || { name: mode };
    modeBtn.dataset.mode = mode;
    modeBtn.innerHTML = `${esc(m.short || m.name)}${chevron}`;
    modeBtn.title = `${m.name}${m.desc ? ": " + m.desc : ""} (⇧Tab)`;
  }

  function currentModel() {
    const list = S.models.length ? S.models : FALLBACK_MODELS;
    const cur = S.meta.model || "default";
    return list.find((x) => x.value === cur) || list.find((x) => cur.includes(x.value)) || null;
  }
  // The modes this model offers: auto mode only where the model has it.
  function availableModes() {
    const m = currentModel();
    return MODES.filter((x) => !x.needsAuto || !m || m.auto !== false);
  }

  function drawModel() {
    const cur = S.meta.model || "default";
    const found = currentModel();
    // "Default (recommended)" says less than the model it stands for.
    const real = found && found.value === "default" && found.description && /^[\w .-]+\d/.exec(found.description.split(" · ")[0]);
    const name = real ? real[0].trim() : found ? found.name.replace(/\s*\(.*\)$/, "") : cur.replace(/^claude-/, "");
    const effort = S.meta.effort && effortLevels().length
      ? `<span class="sub">${esc(EFFORT_NAMES[S.meta.effort] || S.meta.effort)}</span>` : "";
    modelBtn.innerHTML = `${esc(name)}${effort}`;
  }

  // The levels this model takes: Claude's list once it has started, every
  // level before that, none for a model without effort.
  function effortLevels() {
    if (!S.models.length) return ALL_EFFORTS;
    const m = currentModel();
    return m ? m.efforts || [] : ALL_EFFORTS;
  }

  function setMode(mode) {
    post({ chat: "mode", mode });
    S.meta.mode = mode;
    drawMode();
  }

  function updateTitle() {
    const first = thread.querySelector(".user .bubble");
    document.title = first ? firstLine(first.textContent, 48) : "Claude";
  }

  // ---- menus -------------------------------------------------------------

  // A menu of sections: [{title, rows: [{value, name, desc}], current, pick}].
  function openMenu(anchor, sections) {
    menu.innerHTML = "";
    menu.className = "popover menu";
    sections.forEach((sec, k) => {
      if (k) menu.append(h("div", "menu-sep"));
      menu.append(h("div", "menu-title", esc(sec.title)));
      const inline = sec.inline && h("div", "menu-inline");
      sec.rows.forEach((r) => {
        const desc = r.desc || r.description;
        const b = inline
          ? h("button", "menu-pill" + (r.value === sec.current ? " on" : ""), esc(r.name))
          : h("button", "menu-row" + (r.danger ? " danger" : ""), `<span>${esc(r.name)}</span>${desc ? `<small>${esc(desc)}</small>` : ""}${r.value === sec.current ? '<span class="check">✓</span>' : ""}`);
        b.addEventListener("click", () => { closeMenu(); sec.pick(r.value); });
        (inline || menu).append(b);
      });
      if (inline) menu.append(inline);
    });
    menu.hidden = false;
    const wrap = anchor.closest(".composer-wrap").getBoundingClientRect();
    const a = anchor.getBoundingClientRect();
    const w = Math.max(240, menu.offsetWidth);
    menu.style.left = Math.max(8, Math.min(a.right - wrap.left - w, wrap.width - w - 8)) + "px";
    menu.style.bottom = (wrap.bottom - a.top + 6) + "px";
  }
  function closeMenu() { menu.hidden = true; S.modelMenu = null; }
  document.addEventListener("mousedown", (ev) => {
    if (!menu.hidden && !menu.contains(ev.target) && !modeBtn.contains(ev.target) && !modelBtn.contains(ev.target)) closeMenu();
  });

  modeBtn.addEventListener("click", () => {
    if (!menu.hidden) return closeMenu();
    openMenu(modeBtn, [{ title: "Permissions", rows: availableModes(), current: S.meta.mode || "default", pick: setMode }]);
  });
  // ---- model menu --------------------------------------------------------

  // The models picked by name (Opus, Sonnet, …) up top, numbered; the
  // effort and the rest (Default, older and dated models) a level down.
  function modelLists() {
    const all = S.models.length ? S.models : FALLBACK_MODELS;
    const main = all.filter((m) => m.value !== "default" && !/^claude-/.test(m.value));
    return { main, more: all.filter((m) => !main.includes(m)) };
  }
  // The row the current model is: the one named, or with none (or
  // "default") chosen, the main model Default stands for.
  function isCurrent(m) {
    const cur = S.meta.model || "default";
    if (m.value === cur) return true;
    if (cur !== "default") return false;
    const def = (S.models.length ? S.models : FALLBACK_MODELS).find((x) => x.value === "default");
    return !!(def && def.resolved && m.resolved === def.resolved && m.value !== "default" && !/^claude-/.test(m.value));
  }
  function pickModel(v) {
    closeMenu();
    post({ chat: "model", model: v });
    S.meta.model = v;
    drawModel();
    drawMode();
  }
  function pickEffort(v) {
    closeMenu();
    post({ chat: "effort", effort: v });
    S.meta.effort = v;
    drawModel();
  }
  const check = '<svg class="mm-check" viewBox="0 0 16 16"><path d="M3 8.5 6.5 12 13 4.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  const back = '<svg class="mm-back" viewBox="0 0 12 12"><path d="M7.5 2.5 4 6l3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  function mmRow(label, right, onClick, title) {
    const b = h("button", "mm-row", `<span class="mm-name">${label}</span><span class="mm-right">${right}</span>`);
    if (title) b.title = title;
    b.addEventListener("click", onClick);
    menu.append(b);
    return b;
  }
  function openModelMenu(view) {
    S.modelMenu = view;
    menu.innerHTML = "";
    menu.className = "popover menu model-menu";
    const { main, more } = modelLists();
    if (view === "effort" || view === "more") {
      mmRow(`${back}${view === "effort" ? "Effort" : "More models"}`, "", () => openModelMenu("main")).classList.add("mm-head");
      menu.append(h("div", "menu-sep"));
    }
    if (view === "main") {
      main.forEach((m, i) => mmRow(esc(m.name), isCurrent(m) ? check : `<span class="mm-key">${i + 1}</span>`, () => pickModel(m.value), m.description));
      const levels = effortLevels();
      if (levels.length) {
        menu.append(h("div", "menu-sep"));
        const cur = S.meta.effort ? EFFORT_NAMES[S.meta.effort] || S.meta.effort : "Default";
        mmRow("Effort", `<span class="mm-value">${esc(cur)}</span>${chevron}`, () => openModelMenu("effort"));
      }
      if (more.length) {
        menu.append(h("div", "menu-sep"));
        mmRow("More models", chevron, () => openModelMenu("more"));
      }
    } else if (view === "effort") {
      effortLevels().forEach((l) => mmRow(esc(EFFORT_NAMES[l] || l), S.meta.effort === l ? check : "", () => pickEffort(l)));
    } else {
      more.forEach((m) => {
        const desc = m.description ? `<small>${esc(m.description)}</small>` : "";
        mmRow(`${esc(m.name)}${desc}`, m.value === (S.meta.model || "default") ? check : "", () => pickModel(m.value)).classList.add("mm-tall");
      });
    }
    menu.hidden = false;
    const wrap = modelBtn.closest(".composer-wrap").getBoundingClientRect();
    const a = modelBtn.getBoundingClientRect();
    const w = Math.max(280, menu.offsetWidth);
    menu.style.left = Math.max(8, Math.min(a.right - wrap.left - w, wrap.width - w - 8)) + "px";
    menu.style.bottom = (wrap.bottom - a.top + 6) + "px";
  }
  modelBtn.addEventListener("click", () => {
    if (!menu.hidden) return closeMenu();
    warm();
    openModelMenu("main");
  });
  // 1–9 pick a main model while the menu shows them.
  document.addEventListener("keydown", (ev) => {
    if (menu.hidden || S.modelMenu !== "main" || ev.metaKey || ev.ctrlKey || ev.altKey) return;
    const n = Number(ev.key);
    const m = n >= 1 && modelLists().main[n - 1];
    if (!m) return;
    ev.preventDefault();
    ev.stopPropagation();
    pickModel(m.value);
  }, true);

  // ---- slash commands ----------------------------------------------------

  const BUILTIN = [
    { name: "compact", description: "Summarize the conversation to free up context" },
    { name: "clear", description: "Start a new conversation" },
    { name: "cost", description: "Show what this session has used" },
    { name: "context", description: "Show what fills the context window" },
    { name: "review", description: "Review a pull request" },
    { name: "init", description: "Write a CLAUDE.md for this project" },
  ];
  function slashMatches() {
    const v = input.value;
    if (!/^\/[\w:.-]*$/.test(v)) return [];
    const q = v.slice(1).toLowerCase();
    const seen = new Set();
    // Everything Claude offers (built-ins, skills, plugins' and MCP
    // commands), in a list that scrolls; those that start with what you
    // typed first. `__`-names are Claude's own plumbing.
    const all = [...S.commands, ...BUILTIN]
      .filter((c) => !c.name.startsWith("__"))
      .filter((c) => !seen.has(c.name) && seen.add(c.name));
    return all
      .filter((c) => c.name.toLowerCase().includes(q))
      .sort((a, b) => (a.name.toLowerCase().startsWith(q) ? 0 : 1) - (b.name.toLowerCase().startsWith(q) ? 0 : 1));
  }

  // The `@` word the caret is in, if any: [start, query].
  function mentionAt() {
    const before = input.value.slice(0, input.selectionStart);
    const m = /(^|\s)@([^\s@]*)$/.exec(before);
    return m ? [before.length - m[2].length - 1, m[2]] : null;
  }
  let filesTimer = 0;
  function askFiles() {
    const m = mentionAt();
    if (!m) { if (S.popKind === "files") slash.hidden = true; return; }
    clearTimeout(filesTimer);
    filesTimer = setTimeout(() => post({ chat: "files", q: m[1] }), 60);
  }
  function onFiles(e) {
    const m = mentionAt();
    if (!m || m[1] !== e.q) return;
    S.fileItems = e.items || [];
    S.popKind = "files";
    drawPop();
  }

  // What the popover over the composer offers now: commands after `/`,
  // files after `@`.
  function popItems() {
    if (S.popKind === "files") {
      return S.fileItems.map((f) => {
        const cut = f.lastIndexOf("/");
        return { label: f.slice(cut + 1), desc: cut > 0 ? f.slice(0, cut) : "", mono: false, apply: () => pickFile(f), same: false };
      });
    }
    return slashMatches().map((c) => ({
      label: "/" + c.name, desc: c.description || "", apply: () => pickSlash(c),
      same: input.value.trim() === "/" + c.name,
    }));
  }
  function drawPop() {
    const list = popItems();
    if (!list.length) { slash.hidden = true; return; }
    S.slashSel = Math.min(S.slashSel, list.length - 1);
    slash.innerHTML = "";
    slash.dataset.kind = S.popKind || "slash";
    list.forEach((it, k) => {
      const row = h("button", "slash-row" + (k === S.slashSel ? " sel" : ""), `<b>${esc(it.label)}</b><small>${esc(it.desc)}</small>`);
      row.addEventListener("mousedown", (ev) => { ev.preventDefault(); it.apply(); });
      slash.append(row);
    });
    slash.hidden = false;
    const sel = slash.querySelector(".sel");
    if (sel) sel.scrollIntoView({ block: "nearest" });
  }
  function drawSlash() {
    // Claude's own list arrives once it runs; until then, the built-ins.
    warm();
    S.popKind = "slash";
    drawPop();
  }
  function pickSlash(c) {
    input.value = "/" + c.name + " ";
    slash.hidden = true;
    autosize();
    drawSend();
    input.focus();
  }
  function pickFile(f) {
    const m = mentionAt();
    if (!m) return;
    const end = input.selectionStart;
    const ins = "@" + f + " ";
    input.value = input.value.slice(0, m[0]) + ins + input.value.slice(end);
    const caret = m[0] + ins.length;
    input.setSelectionRange(caret, caret);
    slash.hidden = true;
    autosize();
    drawSend();
    input.focus();
  }

  // ---- composer ------------------------------------------------------------

  function autosize() {
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, window.innerHeight * 0.4) + "px";
  }

  function warm() {
    if (!S.warmed) { S.warmed = true; post({ chat: "warm" }); }
  }

  function send() {
    const text = input.value.trim();
    if (!text && !S.images.length) return;
    if (text === "/clear" || text === "/new") {
      post({ chat: "new" });
    } else {
      post({ chat: "send", text, images: S.images.map(({ mediaType, data }) => ({ mediaType, data })) });
    }
    input.value = "";
    S.images = [];
    drawAttachments();
    slash.hidden = true;
    autosize();
    drawSend();
    S.stick = true;
  }

  input.addEventListener("focus", warm);
  input.addEventListener("input", () => {
    autosize();
    drawSend();
    S.slashSel = 0;
    if (mentionAt()) askFiles();
    else drawSlash();
  });
  input.addEventListener("click", () => { if (S.popKind === "files") askFiles(); });
  input.addEventListener("keydown", (ev) => {
    if (!slash.hidden) {
      const items = popItems();
      const n = items.length;
      if (n && (ev.key === "ArrowDown" || ev.key === "ArrowUp")) {
        ev.preventDefault();
        S.slashSel = (S.slashSel + (ev.key === "ArrowDown" ? 1 : n - 1)) % n;
        drawPop();
        return;
      }
      const it = items[S.slashSel];
      if (it && (ev.key === "Tab" || (ev.key === "Enter" && !ev.shiftKey && !it.same))) {
        ev.preventDefault();
        it.apply();
        return;
      }
      if (ev.key === "Escape") { slash.hidden = true; ev.preventDefault(); ev.stopPropagation(); return; }
    }
    if (ev.key === "Enter" && !ev.shiftKey && !ev.isComposing) {
      ev.preventDefault();
      // Enter on an empty composer answers the waiting prompt.
      if (!input.value.trim() && !S.images.length) {
        const ask = [...S.asks.values()].pop();
        const allow = ask && ask.querySelector(".btn.primary");
        if (allow && !ask.querySelector(".question")) allow.click();
        return;
      }
      send();
    } else if (ev.key === "Tab" && ev.shiftKey) {
      ev.preventDefault();
      const cyc = availableModes().filter((m) => m.cycle);
      const k = cyc.findIndex((m) => m.value === (S.meta.mode || "default"));
      setMode(cyc[(k + 1) % cyc.length].value);
    }
  });
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      if (!menu.hidden) { closeMenu(); return; }
      const ask = [...S.asks.values()].pop();
      if (ask && document.activeElement === input && !ask.querySelector(".question")) {
        const deny = [...ask.querySelectorAll(".btn")].find((b) => /Deny|Keep planning/.test(b.textContent));
        if (deny) { deny.click(); return; }
      }
      if (S.state === "running" || S.state === "waiting") post({ chat: "interrupt" });
    }
  });
  sendBtn.addEventListener("click", () => {
    if (sendBtn.classList.contains("stop")) post({ chat: "interrupt" });
    else send();
    input.focus();
  });

  input.addEventListener("paste", (ev) => {
    const files = [...(ev.clipboardData?.items || [])].filter((i) => i.kind === "file" && i.type.startsWith("image/"));
    if (!files.length) return;
    ev.preventDefault();
    files.forEach((it) => addImage(it.getAsFile()));
  });
  document.addEventListener("dragover", (ev) => ev.preventDefault());
  document.addEventListener("drop", (ev) => {
    ev.preventDefault();
    [...(ev.dataTransfer?.files || [])].filter((f) => f.type.startsWith("image/")).forEach(addImage);
  });
  function addImage(file) {
    if (!file) return;
    const r = new FileReader();
    r.onload = () => {
      const url = String(r.result);
      const data = url.slice(url.indexOf(",") + 1);
      S.images.push({ mediaType: file.type, data, url });
      drawAttachments();
      drawSend();
    };
    r.readAsDataURL(file);
  }
  function drawAttachments() {
    attachments.innerHTML = "";
    S.images.forEach((img, k) => {
      const t = h("div", "thumb");
      const i = h("img");
      i.src = img.url;
      const x = h("button", "", "×");
      x.addEventListener("click", () => { S.images.splice(k, 1); drawAttachments(); drawSend(); });
      t.append(i, x);
      attachments.append(t);
    });
    attachments.hidden = !S.images.length;
  }

  // Links open as browser tabs beside this one.
  document.addEventListener("click", (ev) => {
    const a = ev.target.closest && ev.target.closest("a[href]");
    if (!a) return;
    ev.preventDefault();
    post({ chat: "open", url: a.href });
  });

  // ---- the event pump --------------------------------------------------------

  function reset() {
    thread.innerHTML = "";
    S.items.clear();
    S.tools.clear();
    S.asks.clear();
    S.used = 0;
    S.turnTools = [];
    S.turnClosed = true;
    drawContext();
    $("empty").hidden = false;
    document.title = "Claude";
  }

  function handle(e) {
    switch (e.t) {
      case "reset": return reset();
      case "meta": return onMeta(e);
      case "init":
        S.commands = e.commands || [];
        S.models = e.models || [];
        drawModel();
        return;
      case "user": return onUser(e);
      case "text": return onText(e);
      case "thinking": return onThinking(e);
      case "tool": return onTool(e);
      case "tool_result": return onToolResult(e);
      case "permission": return onPermission(e);
      case "permission_done": return resolveAsk(e.reqId, e.allow === false ? "Denied" : "Allowed");
      case "sub": return onSub(e, false);
      case "sub_done": return onSub(e, true);
      case "usage": S.used = e.used; return drawContext();
      case "files": return onFiles(e);
      case "rewind_preview": return onRewindPreview(e);
      case "attach":
        S.images.push({ mediaType: e.mediaType, data: e.data, url: `data:${e.mediaType};base64,${e.data}` });
        drawAttachments();
        drawSend();
        input.focus();
        return;
      case "mention": {
        // At the caret, with a space either side as needed.
        const at = input.selectionStart ?? input.value.length;
        const before = input.value.slice(0, at), after = input.value.slice(at);
        const ins = (before && !/\s$/.test(before) ? " " : "") + e.text + " ";
        input.value = before + ins + after;
        const caret = (before + ins).length;
        input.focus();
        input.setSelectionRange(caret, caret);
        autosize();
        drawSend();
        return;
      }
      case "composer":
        input.value = e.text || "";
        autosize();
        drawSend();
        input.focus();
        input.setSelectionRange(input.value.length, input.value.length);
        return;
      case "note": {
        const el = h("div", "item note" + (e.text.includes("\n") ? " block" : ""));
        el.textContent = e.text;
        return place(null, el);
      }
      case "error": {
        const el = h("div", "item error");
        el.textContent = e.text;
        return place(null, el);
      }
      case "result": {
        closeTurn();
        if (e.error && e.text) {
          const el = h("div", "item error");
          el.textContent = e.text;
          place(null, el);
        } else if (e.ms) {
          const s = e.ms / 1000;
          const t = s < 60 ? `${s.toFixed(s < 10 ? 1 : 0)}s` : `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
          const cost = e.cost ? ` · $${e.cost.toFixed(e.cost < 1 ? 3 : 2)}` : "";
          place(null, h("div", "turn-meta", esc(t + cost)));
        }
        return;
      }
    }
  }

  window.__chat = {
    recv(batch) {
      const stick = S.stick || nearBottom();
      for (const e of batch) {
        try { handle(e); } catch (err) { console.error(err, e); }
      }
      S.stick = stick;
      // A conversation read back has no turn ends; close the last one.
      if (S.state !== "running" && S.state !== "waiting" && !S.turnClosed) closeTurn();
      drawSend();
      tickWorking();
      requestAnimationFrame(follow);
    },
  };

  drawMode();
  drawModel();
  drawSend();
  post({ chat: "ready" });
  input.focus();
})();
