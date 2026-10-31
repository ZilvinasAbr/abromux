// ⌘-click or middle-click on a link: open it in a new browser tab (⇧ to switch
// to it) instead of following it here.
(() => {
  const open = (e) => {
    const a = e.target.closest?.('a[href]');
    if (!a) return;
    const url = typeof a.href === 'string' ? a.href : a.href.baseVal;
    if (!/^https?:/i.test(url)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    window.ipc.postMessage(JSON.stringify({ openTab: url, focus: e.shiftKey }));
  };
  addEventListener('click', (e) => { if (e.button === 0 && e.metaKey) open(e); }, true);
  addEventListener('auxclick', (e) => { if (e.button === 1) open(e); }, true);
})();
