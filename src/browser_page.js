// Runs at document start in every frame of a browser pane (installed by
// browser_native.m). It reports to abromux through the `abromuxPage`
// handler:
//   - the frame's token, so `browser frame` can run scripts in it
//   - console messages, page errors and network requests, in batches
//   - the link under the pointer, for the URL shown at the page's corner
(() => {
  const handler = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.abromuxPage;
  if (!handler || window.__abromuxPage) return;
  const post = handler.postMessage.bind(handler);
  const token = Math.random().toString(36).slice(2) + Date.now().toString(36);
  Object.defineProperty(window, '__abromuxPage', { value: { token }, enumerable: false });
  try { post({ kind: 'hello', token }); } catch (_) {}

  // `browser frame <iframe>` asks the frame who it is with a postMessage,
  // which works across origins; the frame answers natively with its token.
  window.addEventListener('message', ev => {
    const d = ev.data;
    if (d && typeof d === 'object' && typeof d.__abromuxAsk === 'string') {
      ev.stopImmediatePropagation();
      try { post({ kind: 'answer', nonce: d.__abromuxAsk, token }); } catch (_) {}
    }
  }, true);

  // ---- events, batched -------------------------------------------------
  let queue = [], timer = 0;
  const flush = () => {
    timer = 0;
    const items = queue;
    queue = [];
    if (items.length) try { post({ kind: 'events', items }); } catch (_) {}
  };
  const emit = item => {
    if (queue.length >= 1000) return;
    item.ts = Date.now();
    queue.push(item);
    if (!timer) timer = setTimeout(flush, 50);
  };
  const cut = (s, n) => (s.length > n ? s.slice(0, n - 1) + '…' : s);
  const show = v => {
    if (typeof v === 'string') return v;
    if (v instanceof Error) return v.stack ? String(v.stack) : String(v);
    if (v === undefined) return 'undefined';
    if (typeof v === 'function') return `[Function ${v.name || 'anonymous'}]`;
    if (typeof Element !== 'undefined' && v instanceof Element) {
      return `<${v.tagName.toLowerCase()}${v.id ? '#' + v.id : ''}>`;
    }
    try {
      const seen = new WeakSet();
      return JSON.stringify(v, (_, x) => {
        if (typeof x === 'bigint') return x.toString() + 'n';
        if (x && typeof x === 'object') { if (seen.has(x)) return '[Circular]'; seen.add(x); }
        return x;
      }) ?? String(v);
    } catch (_) { return String(v); }
  };

  // ---- console -----------------------------------------------------------
  for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
    const orig = console[level];
    if (typeof orig !== 'function') continue;
    console[level] = function (...args) {
      try { emit({ type: 'console', level, text: cut(args.map(show).join(' '), 4000) }); } catch (_) {}
      return orig.apply(this, args);
    };
  }

  // ---- errors ------------------------------------------------------------
  window.addEventListener('error', ev => {
    // Resource load failures are elements' error events, not ErrorEvents.
    if (!(ev instanceof ErrorEvent)) return;
    const where = ev.filename ? `${ev.filename}:${ev.lineno}:${ev.colno}` : '';
    const stack = ev.error && ev.error.stack ? String(ev.error.stack) : '';
    emit({ type: 'error', text: cut(String(ev.message || 'Error'), 2000), source: where, stack: cut(stack, 4000) });
  }, true);
  window.addEventListener('unhandledrejection', ev => {
    const r = ev.reason;
    emit({
      type: 'error',
      text: cut('Unhandled rejection: ' + (r instanceof Error ? r.message : show(r)), 2000),
      stack: cut(r && r.stack ? String(r.stack) : '', 4000),
    });
  });

  // ---- network -----------------------------------------------------------
  const abs = u => { try { return new URL(u, location.href).href; } catch (_) { return String(u); } };
  const now = () => performance.now();
  if (typeof window.fetch === 'function') {
    const origFetch = window.fetch;
    window.fetch = function (input, init) {
      const start = now();
      const req = typeof Request !== 'undefined' && input instanceof Request ? input : null;
      const method = String((init && init.method) || (req && req.method) || 'GET').toUpperCase();
      const url = abs(req ? req.url : input);
      return origFetch.apply(this, arguments).then(res => {
        emit({ type: 'network', kind: 'fetch', method, url, status: res.status,
          mime: res.headers.get('content-type') || '', ms: Math.round(now() - start) });
        return res;
      }, err => {
        emit({ type: 'network', kind: 'fetch', method, url, status: 0,
          error: String(err && err.message || err), ms: Math.round(now() - start) });
        throw err;
      });
    };
  }
  const XHR = window.XMLHttpRequest && window.XMLHttpRequest.prototype;
  if (XHR) {
    const open = XHR.open, send = XHR.send;
    XHR.open = function (method, url) {
      this.__abromux = { method: String(method || 'GET').toUpperCase(), url: abs(url) };
      return open.apply(this, arguments);
    };
    XHR.send = function () {
      const info = this.__abromux;
      if (info) {
        const start = now();
        this.addEventListener('loadend', () => {
          emit({ type: 'network', kind: 'xhr', method: info.method, url: info.url, status: this.status,
            mime: this.getResponseHeader('content-type') || '', ms: Math.round(now() - start),
            ...(this.status ? {} : { error: 'request failed' }) });
        });
      }
      return send.apply(this, arguments);
    };
  }
  // Everything else the page loads (the document, scripts, styles, images…).
  try {
    const resource = e => {
      if (e.initiatorType === 'fetch' || e.initiatorType === 'xmlhttprequest') return;
      emit({ type: 'network', kind: e.initiatorType || 'other',
        method: 'GET', url: e.name, status: e.responseStatus || null, ms: Math.round(e.duration),
        bytes: e.transferSize || e.encodedBodySize || null });
    };
    new PerformanceObserver(list => list.getEntries().forEach(resource))
      .observe({ type: 'resource', buffered: true });
    // The page's own document, once it has loaded (its entry is complete
    // then). A frame's document is already its parent's `iframe` entry.
    if (window === window.top) {
      window.addEventListener('load', () => setTimeout(() => {
        const nav = performance.getEntriesByType('navigation')[0];
        if (nav) emit({ type: 'network', kind: 'document', method: 'GET', url: nav.name,
          status: nav.responseStatus || null, ms: Math.round(nav.loadEventEnd || nav.duration),
          bytes: nav.transferSize || nav.encodedBodySize || null });
      }, 0), { once: true });
    }
  } catch (_) {}

  // ---- link under the pointer ------------------------------------------
  let hovered = '';
  const hover = url => {
    if (url === hovered) return;
    hovered = url;
    try { post({ kind: 'hover', url }); } catch (_) {}
  };
  const linkOf = ev => {
    for (const n of ev.composedPath ? ev.composedPath() : []) {
      if (n instanceof Element && (n.localName === 'a' || n.localName === 'area') && n.hasAttribute('href')) {
        const href = n.href;
        if (typeof href === 'string' && href && !href.startsWith('javascript:')) return href;
        if (href && typeof href === 'object' && href.baseVal) return abs(href.baseVal);
      }
    }
    return '';
  };
  document.addEventListener('mouseover', ev => hover(linkOf(ev)), true);
  document.addEventListener('mouseout', ev => { if (!ev.relatedTarget) hover(''); }, true);
  window.addEventListener('blur', () => hover(''));
  window.addEventListener('pagehide', () => hover(''));
})();
