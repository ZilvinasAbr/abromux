//! Browser automation for `abromux browser snapshot|click|fill|…`, done the
//! way cmux and Playwright's snapshots do it: injected JS walks the page,
//! hands out element refs (`e1`, `e2`, …) and acts on elements by ref or CSS
//! selector. Events are synthesized in the page, so they aren't "trusted";
//! sites that insist on real input won't react to them.
//!
//! Every script returns `JSON.stringify({result})` or `{error}`, which
//! `parse` turns into a value or an error.

use anyhow::{anyhow, bail};
use serde_json::{Value, json};

/// Installs `window.__abromuxAuto` on first use (each new document starts
/// without it), then runs action `@NAME@` with `@OPTS@`.
const SCRIPT: &str = r#"(() => {
const A = window.__abromuxAuto || (window.__abromuxAuto = {
  refs: new Map(),
  ids: new WeakMap(),
  next: 0,

  // ---- elements --------------------------------------------------------
  refName(t) {
    const m = /^@?(?:ref=)?(e\d+)$/.exec(t);
    return m && m[1];
  },
  find(t) {
    const r = this.refName(t);
    if (r) {
      const e = this.refs.get(r);
      return e && e.isConnected ? e : null;
    }
    try { return document.querySelector(t); } catch (_) { throw new Error(`bad selector ${t}`); }
  },
  el(t) {
    if (!t) throw new Error('missing selector or ref');
    const e = this.find(t);
    if (e) return e;
    throw new Error(this.refName(t) ? `ref ${t} is gone (take a new snapshot)` : `nothing matches ${t}`);
  },
  ref(e) {
    let r = this.ids.get(e);
    if (!r || this.refs.get(r) !== e) {
      r = 'e' + ++this.next;
      this.ids.set(e, r);
      this.refs.set(r, e);
    }
    return r;
  },
  // Not hidden by CSS. Snapshots prune on this alone, since a box with no
  // size can still hold visible children.
  shown(e) {
    if (!e.isConnected) return false;
    const s = getComputedStyle(e);
    // No box of its own; its children are judged on theirs.
    if (s.display === 'contents') return true;
    return e.checkVisibility ? e.checkVisibility({ visibilityProperty: true }) : s.display !== 'none' && s.visibility !== 'hidden';
  },
  // Shown and with a size, as `is visible` and `wait` mean it. A hidden tab
  // lays out at 0×0, so size only counts once the page has a viewport.
  visible(e) {
    if (!this.shown(e)) return false;
    if (!innerWidth || !innerHeight || getComputedStyle(e).display === 'contents') return true;
    const r = e.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  },
  disabled(e) {
    return !!(e.disabled || e.closest('fieldset:disabled') || e.getAttribute('aria-disabled') === 'true');
  },
  editable(e) {
    return e.isContentEditable || (e instanceof HTMLTextAreaElement || e instanceof HTMLInputElement && !/^(button|submit|reset|checkbox|radio|file|image|range|color|hidden)$/.test(e.type)) && !e.readOnly && !this.disabled(e);
  },
  squash(s) { return (s || '').replace(/\s+/g, ' ').trim(); },
  cut(s, n = 100) { return s.length > n ? s.slice(0, n - 1) + '…' : s; },

  // ---- roles and names, roughly as accessibility trees have them -------
  role(e) {
    const explicit = (e.getAttribute('role') || '').trim().split(/\s+/)[0];
    if (explicit) return explicit;
    const t = e.tagName.toLowerCase();
    switch (t) {
      case 'a': return e.hasAttribute('href') ? 'link' : null;
      case 'button': case 'summary': return 'button';
      case 'input': {
        const ty = (e.getAttribute('type') || 'text').toLowerCase();
        return { button: 'button', submit: 'button', reset: 'button', image: 'button', checkbox: 'checkbox', radio: 'radio',
          range: 'slider', number: 'spinbutton', search: 'searchbox', hidden: null, file: 'button', color: 'button' }[ty] ?? 'textbox';
      }
      case 'textarea': return 'textbox';
      case 'select': return e.multiple || e.size > 1 ? 'listbox' : 'combobox';
      case 'option': return 'option';
      case 'img': return e.getAttribute('alt') === '' ? null : 'img';
      case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6': return 'heading';
      case 'nav': return 'navigation';
      case 'main': return 'main';
      case 'header': return e.closest('article,aside,main,nav,section') ? null : 'banner';
      case 'footer': return e.closest('article,aside,main,nav,section') ? null : 'contentinfo';
      case 'aside': return 'complementary';
      case 'form': return 'form';
      case 'ul': case 'ol': return 'list';
      case 'li': return 'listitem';
      case 'table': return 'table';
      case 'tr': return 'row';
      case 'td': return 'cell';
      case 'th': return 'columnheader';
      case 'dialog': return 'dialog';
      case 'p': return 'paragraph';
      case 'progress': return 'progressbar';
      case 'iframe': return 'iframe';
      case 'hr': return 'separator';
      case 'article': return 'article';
      case 'fieldset': return 'group';
      case 'section': return e.hasAttribute('aria-label') || e.hasAttribute('aria-labelledby') ? 'region' : null;
    }
    return e.isContentEditable && !e.parentElement?.isContentEditable ? 'textbox' : null;
  },
  INTERACTIVE: new Set(['link', 'button', 'checkbox', 'radio', 'textbox', 'searchbox', 'combobox', 'listbox', 'option', 'slider',
    'spinbutton', 'switch', 'tab', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'treeitem']),
  // Roles named by their content: their subtree isn't listed.
  LEAF: new Set(['link', 'button', 'heading', 'option', 'tab', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'treeitem',
    'checkbox', 'radio', 'switch', 'img', 'textbox', 'searchbox', 'combobox', 'slider', 'spinbutton', 'progressbar', 'separator', 'iframe']),
  name(e, role) {
    const by = e.getAttribute('aria-labelledby');
    if (by) {
      const t = by.split(/\s+/).map(id => document.getElementById(id)?.innerText || '').join(' ');
      if (this.squash(t)) return this.squash(t);
    }
    const label = e.getAttribute('aria-label');
    if (label && label.trim()) return this.squash(label);
    if (e.labels && e.labels.length) return this.squash([...e.labels].map(l => l.innerText).join(' '));
    if (role === 'img') return this.squash(e.getAttribute('alt') || e.getAttribute('title') || '');
    if (e instanceof HTMLInputElement && /^(button|submit|reset)$/.test(e.type)) return e.value || e.type;
    if (/^(textbox|searchbox|combobox|listbox|spinbutton|slider)$/.test(role)) {
      return this.squash(e.getAttribute('placeholder') || e.getAttribute('title') || '');
    }
    const text = this.LEAF.has(role) || role === 'listitem' && !e.children.length ? this.squash(e.innerText ?? e.textContent) : '';
    return text || this.squash(e.getAttribute('title') || '');
  },
  states(e, role) {
    const out = [];
    if (role === 'heading') out.push('level=' + (+e.getAttribute('aria-level') || +e.tagName[1] || 2));
    const checked = e.getAttribute('aria-checked') ?? (/^(checkbox|radio)$/.test(e.type) ? String(e.checked) : null);
    if (checked === 'true') out.push('checked');
    if (checked === 'mixed') out.push('mixed');
    if (e.getAttribute('aria-expanded') === 'true') out.push('expanded');
    if (e.getAttribute('aria-selected') === 'true' || role === 'option' && e.selected) out.push('selected');
    if (this.disabled(e)) out.push('disabled');
    if (document.activeElement === e) out.push('focused');
    return out;
  },
  value(e, role) {
    if (/^(textbox|searchbox|spinbutton|slider)$/.test(role) && 'value' in e) return e.type === 'password' ? '•••' : e.value;
    if (role === 'textbox' && e.isContentEditable) return e.innerText;
    if (role === 'combobox' && e instanceof HTMLSelectElement) return e.selectedOptions[0]?.label ?? '';
    return '';
  },

  // ---- snapshot --------------------------------------------------------
  // Items are {e, role, name, children} or {text}.
  collect(nodes, opts) {
    const out = [];
    for (const n of nodes) {
      if (n.nodeType === Node.TEXT_NODE) {
        const t = this.squash(n.nodeValue);
        if (t && !opts.interactive) {
          const last = out[out.length - 1];
          if (last && last.text != null) last.text += ' ' + t; else out.push({ text: t });
        }
        continue;
      }
      if (n.nodeType !== Node.ELEMENT_NODE || /^(SCRIPT|STYLE|NOSCRIPT|TEMPLATE|HEAD|META|LINK)$/.test(n.tagName)) continue;
      if (n.getAttribute('aria-hidden') === 'true' || !this.shown(n)) continue;
      let role = this.role(n);
      if (role === 'none' || role === 'presentation' || role === 'generic') role = null;
      const interactive = role && this.INTERACTIVE.has(role) || n.hasAttribute('onclick') || n.tabIndex >= 0 && n.hasAttribute('tabindex');
      const keep = opts.interactive ? interactive : role || interactive;
      const kids = n.shadowRoot ? n.shadowRoot.childNodes : n.childNodes;
      if (!keep) {
        out.push(...this.collect(kids, opts));
        continue;
      }
      role = role || 'generic';
      const item = { e: n, role, name: this.name(n, role), children: [] };
      if (!this.LEAF.has(role)) item.children = this.collect(kids, opts);
      // Compact: drop unnamed containers (keeping what's inside) unless they hold just text.
      if (opts.compact && !interactive && !item.name && role !== 'heading' && !(item.children.length && item.children.every(c => c.text != null))) {
        out.push(...item.children);
        continue;
      }
      out.push(item);
    }
    return out;
  },
  render(items, depth, lines, opts) {
    const pad = '  '.repeat(depth);
    for (const it of items) {
      if (lines.length >= opts.max) return;
      if (it.text != null) {
        lines.push(`${pad}- text: ${JSON.stringify(this.cut(it.text, 200))}`);
        continue;
      }
      let line = `${pad}- ${it.role}`;
      if (it.name) line += ' ' + JSON.stringify(this.cut(it.name));
      const st = this.states(it.e, it.role);
      if (st.length) line += ` [${st.join('] [')}]`;
      line += ` [ref=${this.ref(it.e)}]`;
      const value = this.value(it.e, it.role);
      if (value) line += ': ' + JSON.stringify(this.cut(value));
      const kids = it.children;
      if (!value && kids.length === 1 && kids[0].text != null) {
        if (!it.name || it.name !== kids[0].text) line += ': ' + JSON.stringify(this.cut(kids[0].text, 200));
        lines.push(line);
        continue;
      }
      lines.push(line);
      if (it.role === 'link' && it.e.getAttribute('href')) lines.push(`${pad}  - /url: ${this.cut(it.e.getAttribute('href'), 200)}`);
      this.render(kids, depth + 1, lines, opts);
    }
  },
  snapshot(o) {
    this.refs.clear();
    this.ids = new WeakMap();
    this.next = 0;
    const root = o.selector ? this.el(o.selector) : document.body;
    if (!root) return { snapshot: '', url: location.href, title: document.title };
    const opts = { interactive: !!o.interactive, compact: !!o.compact, max: 3000 };
    const lines = [];
    this.render(this.collect([root], opts), 0, lines, opts);
    if (lines.length >= opts.max) lines.push(`- … (stopped at ${opts.max} lines; use --selector or --interactive)`);
    return { snapshot: lines.join('\n'), url: location.href, title: document.title, refs: this.next };
  },

  // ---- input -----------------------------------------------------------
  point(e) {
    e.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
    const r = e.getBoundingClientRect();
    return { clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 };
  },
  mouse(e, type, at, extra = {}) {
    const init = { bubbles: true, cancelable: true, composed: true, view: window, button: 0, ...at, ...extra };
    const C = type.startsWith('pointer') ? PointerEvent : MouseEvent;
    return e.dispatchEvent(new C(type, type.startsWith('pointer') ? { pointerId: 1, pointerType: 'mouse', isPrimary: true, ...init } : init));
  },
  hover(o) {
    const e = this.el(o.selector), at = this.point(e);
    for (const t of ['pointerover', 'pointerenter', 'mouseover', 'mouseenter', 'pointermove', 'mousemove']) this.mouse(e, t, at);
    return null;
  },
  clickEl(e, count = 1) {
    if (this.disabled(e)) throw new Error('element is disabled');
    const at = this.point(e);
    const hit = document.elementFromPoint(at.clientX, at.clientY);
    // A covering element (an overlay, a label's text) gets the click, as a real one would.
    const target = hit && (hit === e || e.contains(hit) || hit.contains(e) || hit.closest('label')?.control === e) ? hit : e;
    this.mouse(target, 'pointerover', at); this.mouse(target, 'mouseover', at);
    for (let i = 1; i <= count; i++) {
      this.mouse(target, 'pointerdown', at, { buttons: 1, detail: i });
      this.mouse(target, 'mousedown', at, { buttons: 1, detail: i });
      if (i === 1 && e.focus) e.focus({ preventScroll: true });
      this.mouse(target, 'pointerup', at, { detail: i });
      this.mouse(target, 'mouseup', at, { detail: i });
      this.mouse(target, 'click', at, { detail: i });
    }
    if (count === 2) this.mouse(target, 'dblclick', at, { detail: 2 });
  },
  click(o) { this.clickEl(this.el(o.selector), o.count || 1); return null; },
  dblclick(o) { this.clickEl(this.el(o.selector), 2); return null; },
  focus(o) { const e = this.el(o.selector); e.scrollIntoView({ block: 'center' }); e.focus(); return null; },
  scrollIntoView(o) { this.el(o.selector).scrollIntoView({ block: 'center', inline: 'center' }); return null; },
  isChecked(e) {
    const a = e.getAttribute('aria-checked');
    return a != null ? a === 'true' : !!e.checked;
  },
  check(o) {
    const e = this.el(o.selector), want = o.checked !== false;
    if (this.isChecked(e) !== want) this.clickEl(e);
    if (this.isChecked(e) !== want) throw new Error(`clicked, but it's still ${want ? 'unchecked' : 'checked'}`);
    return { checked: want };
  },
  // Set a value so frameworks notice: React tracks the instance's value
  // setter, so go through the prototype's.
  setValue(e, v) {
    const proto = e instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : e instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, 'value').set.call(e, v);
  },
  insert(e, text) {
    if (e.isContentEditable) {
      document.execCommand('insertText', false, text);
      return;
    }
    if (!('value' in e)) return;
    const start = e.selectionStart ?? e.value.length, end = e.selectionEnd ?? e.value.length;
    try { e.setRangeText(text, start, end, 'end'); } catch (_) { this.setValue(e, e.value + text); }
    e.dispatchEvent(new InputEvent('input', { bubbles: true, composed: true, inputType: 'insertText', data: text }));
  },
  fill(o) {
    const e = this.el(o.selector);
    if (!this.editable(e)) throw new Error('element is not an editable field');
    e.scrollIntoView({ block: 'center' });
    e.focus();
    if (e.isContentEditable) {
      document.getSelection().selectAllChildren(e);
      document.execCommand(o.text ? 'insertText' : 'delete', false, o.text);
    } else {
      this.setValue(e, o.text);
      e.dispatchEvent(new InputEvent('input', { bubbles: true, composed: true, inputType: 'insertText', data: o.text }));
      e.dispatchEvent(new Event('change', { bubbles: true }));
    }
    return null;
  },
  key(e, type, k, mods) {
    const code = k.length === 1 ? (/[a-z]/i.test(k) ? 'Key' + k.toUpperCase() : /\d/.test(k) ? 'Digit' + k : '') : k;
    return e.dispatchEvent(new KeyboardEvent(type, { key: k, code, bubbles: true, cancelable: true, composed: true, ...mods }));
  },
  active() {
    let e = document.activeElement || document.body;
    while (e.shadowRoot?.activeElement) e = e.shadowRoot.activeElement;
    return e;
  },
  type(o) {
    if (o.selector) { const e = this.el(o.selector); e.scrollIntoView({ block: 'center' }); e.focus(); }
    for (const ch of [...(o.text || '')]) {
      const e = this.active(), k = ch === '\n' ? 'Enter' : ch;
      if (!this.key(e, 'keydown', k, {})) continue;
      if (ch === '\n') { this.enter(e); } else {
        this.key(e, 'keypress', k, {});
        if (this.editable(e)) this.insert(e, ch);
      }
      this.key(e, 'keyup', k, {});
    }
    if (this.active() && 'value' in this.active()) this.active().dispatchEvent(new Event('change', { bubbles: true }));
    return null;
  },
  enter(e) {
    if (e instanceof HTMLTextAreaElement || e.isContentEditable) return this.insert(e, '\n');
    if (e instanceof HTMLInputElement && e.form) {
      const submit = e.form.querySelector('[type=submit],button:not([type])');
      return submit ? submit.click() : e.form.requestSubmit ? e.form.requestSubmit() : e.form.submit();
    }
    if (e instanceof HTMLButtonElement || e instanceof HTMLAnchorElement || e.getAttribute('role') === 'button') e.click();
  },
  KEYS: { enter: 'Enter', return: 'Enter', tab: 'Tab', escape: 'Escape', esc: 'Escape', backspace: 'Backspace', delete: 'Delete',
    space: ' ', up: 'ArrowUp', down: 'ArrowDown', left: 'ArrowLeft', right: 'ArrowRight', arrowup: 'ArrowUp', arrowdown: 'ArrowDown',
    arrowleft: 'ArrowLeft', arrowright: 'ArrowRight', home: 'Home', end: 'End', pageup: 'PageUp', pagedown: 'PageDown' },
  press(o) {
    if (o.selector) this.el(o.selector).focus();
    const parts = String(o.key).split(/[+-](?=.)/);
    let k = parts.pop();
    const mods = {};
    for (const m of parts.map(p => p.toLowerCase())) {
      if (m === 'ctrl' || m === 'control') mods.ctrlKey = true;
      else if (m === 'shift') mods.shiftKey = true;
      else if (m === 'alt' || m === 'option' || m === 'opt') mods.altKey = true;
      else if (m === 'cmd' || m === 'meta' || m === 'command' || m === 'super') mods.metaKey = true;
      else throw new Error(`unknown modifier ${m}`);
    }
    k = this.KEYS[k.toLowerCase()] || (k.length === 1 ? k : k[0].toUpperCase() + k.slice(1));
    if (k.length === 1 && mods.shiftKey) k = k.toUpperCase();
    const e = this.active();
    if (this.key(e, 'keydown', k, mods)) {
      // What the browser would do by default.
      const plain = !mods.ctrlKey && !mods.metaKey && !mods.altKey;
      if (k === 'Enter' && plain) this.enter(e);
      else if (k === ' ' && plain && (e instanceof HTMLButtonElement || /^(checkbox|radio)$/.test(e.type))) e.click();
      else if (k === 'Tab' && !mods.ctrlKey && !mods.metaKey) this.tab(e, mods.shiftKey);
      else if (k === 'Backspace' && plain && this.editable(e)) {
        if (e.isContentEditable) document.execCommand('delete');
        else if ('value' in e) {
          const s = e.selectionStart ?? e.value.length, en = e.selectionEnd ?? s;
          if (s !== en || s > 0) { e.setRangeText('', s === en ? s - 1 : s, en, 'end'); e.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward' })); }
        }
      } else if (k.length === 1 && plain && this.editable(e)) {
        this.key(e, 'keypress', k, mods);
        this.insert(e, k);
      } else if (mods.metaKey && k.toLowerCase() === 'a' && this.editable(e)) e.select ? e.select() : document.execCommand('selectAll');
    }
    this.key(e, 'keyup', k, mods);
    return null;
  },
  tab(from, back) {
    const all = [...document.querySelectorAll('a[href],button,input,select,textarea,[tabindex],[contenteditable=""],[contenteditable=true]')]
      .filter(e => e.tabIndex >= 0 && !this.disabled(e) && this.visible(e));
    if (!all.length) return;
    const i = all.indexOf(from);
    const next = i < 0 ? (back ? all.length - 1 : 0) : (i + (back ? -1 : 1) + all.length) % all.length;
    all[next].focus();
  },
  select(o) {
    const e = this.el(o.selector);
    if (!(e instanceof HTMLSelectElement)) throw new Error('element is not a <select>');
    const want = [].concat(o.values || []);
    const picked = [...e.options].filter(op => want.some(w => op.value === w || op.label === w || this.squash(op.text) === w));
    if (!picked.length) throw new Error(`no option matches ${want.join(', ')}`);
    for (const op of e.options) op.selected = e.multiple ? picked.includes(op) : op === picked[0];
    e.dispatchEvent(new Event('input', { bubbles: true }));
    e.dispatchEvent(new Event('change', { bubbles: true }));
    return picked.map(op => op.value);
  },
  scroll(o) {
    const dx = +o.dx || 0, dy = +o.dy || 0;
    if (o.selector) this.el(o.selector).scrollBy({ left: dx, top: dy, behavior: 'instant' });
    else window.scrollBy({ left: dx, top: dy, behavior: 'instant' });
    return { x: scrollX, y: scrollY };
  },

  // ---- reading ---------------------------------------------------------
  get(o) {
    const e = () => this.el(o.selector);
    switch (o.what) {
      case 'url': return location.href;
      case 'title': return document.title;
      case 'text': return o.selector ? e().innerText : document.body?.innerText ?? '';
      case 'html': return o.selector ? e().outerHTML : document.documentElement.outerHTML;
      case 'value': { const x = e(); return 'value' in x ? x.value : x.innerText; }
      case 'attr': if (!o.name) throw new Error('get attr needs an attribute name'); return e().getAttribute(o.name);
      case 'count': {
        if (!o.selector) throw new Error('get count needs a selector');
        if (this.refName(o.selector)) return this.find(o.selector) ? 1 : 0;
        try { return document.querySelectorAll(o.selector).length; } catch (_) { throw new Error(`bad selector ${o.selector}`); }
      }
      case 'box': { const r = e().getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height }; }
      case 'styles': {
        const s = getComputedStyle(e());
        const props = o.names && o.names.length ? o.names : ['display', 'visibility', 'opacity', 'color', 'background-color', 'font-size', 'font-weight', 'position', 'width', 'height'];
        return Object.fromEntries(props.map(p => [p, s.getPropertyValue(p)]));
      }
    }
    throw new Error(`can't get ${o.what} (url, title, text, html, value, attr, count, box or styles)`);
  },
  is(o) {
    const e = this.find(o.selector || '');
    if (!o.selector) throw new Error('missing selector or ref');
    switch (o.state) {
      case 'visible': return !!e && this.visible(e);
      case 'hidden': return !e || !this.visible(e);
      case 'enabled': return !!e && !this.disabled(e);
      case 'disabled': return !!e && this.disabled(e);
      case 'checked': return !!e && this.isChecked(e);
      case 'editable': return !!e && this.editable(e);
      case 'focused': return !!e && this.active() === e;
    }
    throw new Error(`unknown state ${o.state} (visible, hidden, enabled, disabled, checked, editable or focused)`);
  },
  findBy(o) {
    const v = String(o.value ?? ''), exact = !!o.exact;
    const match = s => { s = this.squash(s); return exact ? s === v : s.toLowerCase().includes(v.toLowerCase()); };
    const all = [...document.querySelectorAll('body *')].filter(e => this.shown(e));
    let hits;
    switch (o.by) {
      case 'role': hits = all.filter(e => this.role(e) === v && (!o.name || (exact ? this.name(e, v) === o.name : this.name(e, v).toLowerCase().includes(o.name.toLowerCase())))); break;
      case 'text': hits = all.filter(e => [...e.childNodes].some(n => n.nodeType === Node.TEXT_NODE && match(n.nodeValue)) || !e.children.length && match(e.innerText)); break;
      case 'label': hits = all.filter(e => e.labels && [...e.labels].some(l => match(l.innerText)) || match(e.getAttribute('aria-label') || '\u0000')); break;
      case 'placeholder': hits = all.filter(e => match(e.getAttribute('placeholder') || '\u0000')); break;
      case 'alt': hits = all.filter(e => match(e.getAttribute('alt') || '\u0000')); break;
      case 'title': hits = all.filter(e => match(e.getAttribute('title') || '\u0000')); break;
      case 'testid': hits = all.filter(e => e.getAttribute('data-testid') === v); break;
      default: throw new Error(`can't find by ${o.by} (role, text, label, placeholder, alt, title or testid)`);
    }
    return hits.slice(0, 50).map(e => {
      const role = this.role(e) || e.tagName.toLowerCase();
      return { ref: this.ref(e), role, name: this.cut(this.name(e, role) || this.squash(e.innerText)) };
    });
  },
  // For `wait`: whether the selector is in the state wanted.
  waitFor(o) {
    const e = this.find(o.selector);
    switch (o.state || 'visible') {
      case 'attached': return !!e;
      case 'detached': return !e;
      case 'visible': return !!e && this.visible(e);
      case 'hidden': return !e || !this.visible(e);
    }
    throw new Error(`unknown state ${o.state} (visible, hidden, attached or detached)`);
  },

  run(name, o) {
    try {
      const f = { snapshot: this.snapshot, click: this.click, dblclick: this.dblclick, hover: this.hover, focus: this.focus,
        check: this.check, scroll_into_view: this.scrollIntoView, fill: this.fill, type: this.type, press: this.press,
        select: this.select, scroll: this.scroll, get: this.get, is: this.is, find: this.findBy, wait_for: this.waitFor }[name];
      if (!f) throw new Error(`unknown action ${name}`);
      return JSON.stringify({ result: f.call(this, o) ?? null });
    } catch (x) {
      return JSON.stringify({ error: String(x && x.message || x) });
    }
  },
});
return A.run(@NAME@, @OPTS@);
})()"#;

/// Escape a JSON value for splicing into JS: JSON is valid JS, except that
/// U+2028/U+2029 ended string literals before ES2019.
fn js(v: &Value) -> String {
    v.to_string()
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// The script running action `name` (snapshot, click, fill, …) with `opts`.
pub fn script(name: &str, opts: &Value) -> String {
    SCRIPT
        .replacen("@NAME@", &js(&json!(name)), 1)
        .replacen("@OPTS@", &js(opts), 1)
}

/// `wait --js`: the expression, run as is in the page and made a boolean.
pub fn condition(expr: &str) -> String {
    format!(
        "(() => {{ try {{ return JSON.stringify({{ result: !!(\n{expr}\n) }}); }} catch (x) {{ return JSON.stringify({{ error: String(x && x.message || x) }}); }} }})()"
    )
}

/// `wait --text`, `--url` and `--load`, as conditions.
pub fn page_condition(
    text: Option<&str>,
    url: Option<&str>,
    load: Option<&str>,
) -> anyhow::Result<String> {
    let mut parts = vec![];
    if let Some(t) = text {
        parts.push(format!(
            "(document.body?.innerText ?? '').includes({})",
            js(&json!(t))
        ));
    }
    if let Some(u) = url {
        parts.push(format!("location.href.includes({})", js(&json!(u))));
    }
    match load {
        None => {}
        Some("load" | "complete") => parts.push("document.readyState === 'complete'".into()),
        Some("domcontentloaded" | "interactive") => {
            parts.push("document.readyState !== 'loading'".into())
        }
        Some(l) => bail!("--load must be interactive or complete, got {l:?}"),
    }
    Ok(condition(&parts.join(" && ")))
}

/// A script's result: the value, or the page's error.
pub fn parse(out: &str) -> anyhow::Result<Value> {
    // wry hands back the JSON of the value, here a JSON string.
    let inner: String = serde_json::from_str(out)
        .map_err(|_| anyhow!("the page didn't run the script (is it still loading?)"))?;
    let mut v: Value = serde_json::from_str(&inner)?;
    match v.get("error").and_then(Value::as_str) {
        Some(e) => bail!("{e}"),
        None => Ok(v["result"].take()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_safe_js_literals() {
        let s = script(
            "fill",
            &json!({ "selector": "#q", "text": "a\"b\u{2028})</script>" }),
        );
        assert!(
            s.contains(r##"A.run("fill", {"selector":"#q","text":"a\"b\u2028)</script>"})"##),
            "{}",
            &s[s.len() - 120..]
        );
        assert!(!s.contains("@NAME@") && !s.contains("@OPTS@"));
    }

    #[test]
    fn parses_results_and_errors() {
        assert_eq!(parse(r#""{\"result\":3}""#).unwrap(), json!(3));
        assert_eq!(parse(r#""{\"result\":null}""#).unwrap(), Value::Null);
        assert_eq!(
            parse(r##""{\"error\":\"nothing matches #x\"}""##)
                .unwrap_err()
                .to_string(),
            "nothing matches #x"
        );
        assert!(parse("").is_err());
    }

    #[test]
    fn page_conditions() {
        let c = page_condition(Some("Done"), None, Some("complete")).unwrap();
        assert!(
            c.contains(r#"includes("Done") && document.readyState === 'complete'"#),
            "{c}"
        );
        assert!(page_condition(None, None, Some("soon")).is_err());
    }
}
