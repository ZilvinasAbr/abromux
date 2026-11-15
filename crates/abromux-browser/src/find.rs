//! Find in page for browser panes, done like cmux: injected JS wraps each
//! match in `<mark>` and reports `{"total":N,"current":I}` (I is -1 before
//! a match is selected). See docs/browser-find.md.

/// Installs `window.__abromuxFind` on first use (each new document starts
/// without it), then runs `{call}` on it and returns the count.
const SCRIPT: &str = r#"(() => {
const F = window.__abromuxFind || (window.__abromuxFind = {
  marks: [],
  index: -1,
  count() { return { total: this.marks.length, current: this.index }; },
  clear() {
    for (const m of this.marks) {
      const p = m.parentNode;
      if (!p) continue;
      p.replaceChild(document.createTextNode(m.textContent), m);
      p.normalize();
    }
    this.marks = [];
    this.index = -1;
    document.getElementById('__abromux-find-style')?.remove();
    return this.count();
  },
  search(q) {
    this.clear();
    if (!q || !document.body) return this.count();
    const needle = q.toLowerCase();
    const skip = /^(SCRIPT|STYLE|NOSCRIPT|TEMPLATE|IFRAME|SVG)$/;
    const hidden = new Map();
    const isHidden = el => {
      if (!el || el === document.body) return false;
      if (hidden.has(el)) return hidden.get(el);
      const s = getComputedStyle(el);
      const h = skip.test(el.tagName.toUpperCase()) || el.getAttribute('aria-hidden') === 'true'
        || s.display === 'none' || s.visibility === 'hidden' || isHidden(el.parentElement);
      hidden.set(el, h);
      return h;
    };
    const nodes = [];
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n; (n = walker.nextNode()); ) {
      if (n.nodeValue.toLowerCase().includes(needle) && !isHidden(n.parentElement)) nodes.push(n);
    }
    for (const n of nodes) {
      const text = n.nodeValue, lower = text.toLowerCase(), frag = document.createDocumentFragment();
      let at = 0, i;
      while ((i = lower.indexOf(needle, at)) !== -1) {
        frag.append(text.slice(at, i));
        const m = document.createElement('mark');
        m.className = '__abromux-find';
        m.textContent = text.slice(i, i + needle.length);
        frag.append(m);
        this.marks.push(m);
        at = i + needle.length;
      }
      frag.append(text.slice(at));
      n.parentNode.replaceChild(frag, n);
    }
    if (this.marks.length) {
      const style = document.createElement('style');
      style.id = '__abromux-find-style';
      style.textContent = 'mark.__abromux-find{background:#facc15;color:#000;border-radius:2px}'
        + 'mark.__abromux-find.__abromux-find-current{background:#f97316;color:#fff}';
      document.head.append(style);
      this.select(0);
    }
    return this.count();
  },
  select(i) {
    // The page dropped some marks (re-rendered), so the list is stale.
    if (this.marks.some(m => !m.isConnected)) {
      this.marks = [];
      this.index = -1;
      return;
    }
    this.marks[this.index]?.classList.remove('__abromux-find-current');
    const n = this.marks.length;
    this.index = ((i % n) + n) % n;
    const m = this.marks[this.index];
    m.classList.add('__abromux-find-current');
    m.scrollIntoView({ block: 'center', behavior: 'smooth' });
  },
  step(d) {
    if (this.marks.length) this.select(this.index < 0 ? 0 : this.index + d);
    return this.count();
  },
});
return F.{call};
})()"#;

fn script(call: &str) -> String {
    SCRIPT.replace("{call}", call)
}

/// Highlight every match of `query` (case-insensitive) and select the first.
pub fn search(query: &str) -> String {
    // A JSON string is a valid JS string literal, except that U+2028/U+2029
    // ended JS string literals before ES2019.
    let q = serde_json::to_string(query)
        .unwrap()
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    script(&format!("search({q})"))
}

/// Select the next (`forward`) or previous match, wrapping around.
pub fn step(forward: bool) -> String {
    script(if forward { "step(1)" } else { "step(-1)" })
}

/// Remove the highlights.
pub fn clear() -> String {
    script("clear()")
}

pub const SELECTION: &str = "window.getSelection().toString()";

/// `(total, selected)` from a script's JSON result.
pub fn parse_count(json: &str) -> Option<(usize, Option<usize>)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let total = v["total"].as_u64()? as usize;
    let current = v["current"].as_i64()?;
    Some((total, usize::try_from(current).ok().filter(|&i| i < total)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_is_a_safe_js_literal() {
        let s = search("a\"b\\c\n\u{2028}</script>");
        assert!(s.contains(r#"search("a\"b\\c\n\u2028</script>")"#), "{s}");
        assert!(!s.contains("{call}"));
    }

    #[test]
    fn parses_counts() {
        assert_eq!(
            parse_count(r#"{"total":3,"current":1}"#),
            Some((3, Some(1)))
        );
        assert_eq!(parse_count(r#"{"total":2,"current":-1}"#), Some((2, None)));
        assert_eq!(parse_count(r#"{"total":0,"current":-1}"#), Some((0, None)));
        assert_eq!(parse_count(""), None);
    }
}
