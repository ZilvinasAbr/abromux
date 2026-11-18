// WebAuthn on Chromium pages (backend/chromium.rs), in every frame before
// the page's scripts. Chromium does the work itself (security keys over
// USB/NFC, phones over Bluetooth), but a page in an app's view gets CEF's
// "Alloy" style, which has none of Chrome's WebAuthn dialogs. Without them:
//   - nothing tells the user to touch their key, and nothing cancels;
//   - a request that times out never settles (Chrome waits for its dialog
//     to be dismissed first);
//   - passkey autofill (`mediation: "conditional"`) has no list to show, and
//     its pending request blocks every other one ("A request is already
//     pending").
// So: a small card with Cancel while a request runs, the timeout enforced
// with an AbortController (aborting settles at once), and autofill requests
// kept from Chromium (they wait for their signal, as if nothing was picked).
(() => {
  const creds = navigator.credentials;
  if (!creds || !window.PublicKeyCredential || window.__abromuxWebAuthn) return;
  Object.defineProperty(window, '__abromuxWebAuthn', { value: true });

  const DEFAULT_TIMEOUT = 300000; // Chrome's longest
  const create = creds.create.bind(creds);
  const get = creds.get.bind(creds);

  // ---- the card -----------------------------------------------------------
  let card = null;
  const show = (text, cancel) => {
    hide();
    const host = document.createElement('div');
    host.style.cssText = 'all: initial; position: fixed; z-index: 2147483647; top: 12px; left: 50%; transform: translateX(-50%);';
    const root = host.attachShadow({ mode: 'closed' });
    root.innerHTML = `
      <style>
        .c { font: 13px -apple-system, BlinkMacSystemFont, sans-serif; color: #eee;
             background: rgba(32,32,34,.97); border: 1px solid rgba(255,255,255,.12);
             border-radius: 10px; padding: 12px 14px; display: flex; gap: 14px;
             align-items: center; box-shadow: 0 6px 24px rgba(0,0,0,.35); max-width: 440px; }
        .t b { display: block; font-weight: 600; margin-bottom: 2px; }
        .t span { color: #aaa; }
        button { font: inherit; color: #eee; background: rgba(255,255,255,.1);
                 border: 1px solid rgba(255,255,255,.18); border-radius: 6px;
                 padding: 4px 12px; cursor: pointer; }
        button:hover { background: rgba(255,255,255,.18); }
      </style>
      <div class="c" role="dialog" aria-live="polite">
        <div class="t"><b></b><span>Touch your security key, or use your phone if this site offers it.</span></div>
        <button type="button">Cancel</button>
      </div>`;
    root.querySelector('b').textContent = text;
    root.querySelector('button').addEventListener('click', cancel);
    (document.body || document.documentElement).appendChild(host);
    card = host;
  };
  const hide = () => {
    if (card) card.remove();
    card = null;
  };

  // ---- requests -----------------------------------------------------------
  const notAllowed = () =>
    new DOMException('The operation either timed out or was not allowed.', 'NotAllowedError');

  // Run `call` with a signal that aborts on the page's signal, on Cancel,
  // and at the request's timeout; the card shows meanwhile.
  const guarded = (options, title, call) => {
    const pk = options.publicKey;
    const ctl = new AbortController();
    let why = null;
    const stop = reason => {
      if (ctl.signal.aborted) return;
      why = reason;
      ctl.abort();
    };
    const page = options.signal;
    if (page) {
      if (page.aborted) return Promise.reject(page.reason ?? new DOMException('Aborted', 'AbortError'));
      page.addEventListener('abort', () => stop('page'), { once: true });
    }
    const ms = Math.min(Math.max(Number(pk.timeout) || DEFAULT_TIMEOUT, 10000), DEFAULT_TIMEOUT);
    const timer = setTimeout(() => stop('timeout'), ms);
    show(title, () => stop('cancel'));
    return call({ ...options, signal: ctl.signal })
      .catch(e => {
        // A timeout or Cancel is NotAllowedError, as in Chrome; the page's
        // own abort stays its abort.
        if (why === 'timeout' || why === 'cancel') throw notAllowed();
        if (why === 'page' && page) throw page.reason ?? e;
        throw e;
      })
      .finally(() => {
        clearTimeout(timer);
        hide();
      });
  };

  const hostName = () => location.hostname || 'this site';

  creds.create = function (options) {
    if (!options || !options.publicKey) return create(options);
    return guarded(options, `Create a passkey for ${hostName()}`, create);
  };

  creds.get = function (options) {
    if (!options || !options.publicKey) return get(options);
    if (options.mediation === 'conditional') {
      // Autofill: nothing to pick from here. Wait for the page to give up.
      return new Promise((_, reject) => {
        const s = options.signal;
        if (!s) return;
        if (s.aborted) return reject(s.reason);
        s.addEventListener('abort', () => reject(s.reason), { once: true });
      });
    }
    return guarded(options, `Sign in to ${hostName()} with a passkey`, get);
  };

  // No autofill list, so sites use their sign-in button instead.
  try {
    PublicKeyCredential.isConditionalMediationAvailable = () => Promise.resolve(false);
  } catch (_) {}
})();
