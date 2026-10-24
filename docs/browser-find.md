# Find in page for browser panes

Terminal panes have ⌘F (see `src/terminal.rs`). Browser panes don't yet. This is how cmux does it, and how we could copy it.

## How cmux does it

WKWebView has no find bar an app can drive, so cmux searches the page with injected JavaScript and draws its own bar. The code (as of cmux `dea5e18`):

| File | Role |
| --- | --- |
| `Packages/macOS/CmuxBrowser/Sources/CmuxBrowser/Find/Scripts/BrowserFindScript.swift` | The four JS snippets: `search(query)`, `next()`, `previous()`, `clear()` |
| `Packages/macOS/CmuxBrowser/Sources/CmuxBrowser/Find/Service/BrowserFindService.swift` | Runs a snippet with `evaluateJavaScript` and parses the result |
| `Packages/macOS/CmuxBrowser/Sources/CmuxBrowser/Find/Values/BrowserFindMatchCount.swift` | Parses the `{"total":N,"current":M}` the snippets return |
| `Sources/Find/BrowserFindWebViewEvaluator.swift` | Connects the service to the panel's current `WKWebView` |
| `Sources/Find/BrowserSearchOverlay.swift` | The SwiftUI find bar |
| `Sources/BrowserWindowPortal.swift` (`setSearchOverlay`) | Mounts the bar as an `NSHostingView` on top of the webview |

### The scripts

Each snippet is a self-contained IIFE that returns a JSON string:

- **`search(query)`**
  1. Clears any old highlights.
  2. Walks the visible text nodes of `document.body` with a `TreeWalker`. It skips `SCRIPT`, `STYLE`, `NOSCRIPT`, `TEMPLATE`, `IFRAME` and `SVG` elements, anything with `aria-hidden="true"`, and anything `display: none` or `visibility: hidden` up the parent chain.
  3. Finds case-insensitive matches with `toLowerCase().indexOf` and splits each text node, wrapping every match in `<mark class="__cmux-find">`.
  4. Keeps the marks in `window.__cmuxFindMatches` and the current index in `window.__cmuxFindIndex`.
  5. Marks the first match current (`__cmux-find-current`) and `scrollIntoView({block: 'center', behavior: 'smooth'})`s it.
  6. Injects a `<style id="__cmux-find-style">`: yellow `#facc15` with black text for matches, orange `#f97316` with white text for the current one.
  7. Returns `{"total":N,"current":0}`.
- **`next()` / `previous()`:** move the current class by ±1, wrapping, and scroll to the new match. If the page has since dropped a mark (`!isConnected`), they reset to zero matches rather than throw.
- **`clear()`:** replaces each mark with a text node, calls `parent.normalize()` to merge the text back together, removes the stylesheet and resets the globals.
- **The query** is spliced into a JS string literal after escaping `\`, `"`, `\n`, `\r`, `\t`, `\0`, U+2028 and U+2029.

### The bar

- A 180 px text field with a `k/N` count (`-/N` before any match is selected, `0/0` with none), plus ↑ (next), ↓ (previous) and ✕ buttons.
- Return goes to the next match, Shift+Return to the previous one, and Esc closes the bar.
- The bar can be dragged to any corner.
- It's an `NSHostingView` added as a sibling above the `WKWebView` in the panel's portal view, so it floats over the page.
- If the needle is empty, cmux skips the search, clears the highlights and resets the count.

## How to do it in AbroMux

**The scripts port almost as-is.**
- `BrowserView::eval` (`src/browser.rs`) already runs JS and returns the JSON result through a callback on a wry thread. Send the result back to the main thread over a channel, the same way `BrowserView::new` handles title and URL updates.
- Put the four snippets in a `browser_find.rs`, with the escaping as a Rust function. `serde_json::to_string(&query)` does the same job, plus U+2028/U+2029.
- Use our own class names (`__abromux-find…`).

**The bar is the one real difference.**
- cmux floats the bar over the page. We can't: the WKWebView is a native view on top of gpui's Metal layer, so anything gpui draws ends up underneath it (see "Popovers over webviews" in `missing-cmux-features.md`).
- Options, best first:
  1. **Put the bar in the pane's chrome.** Show a 32 px row under the address bar while find is open, and let the webview shrink to fit. It's pure gpui, so we can reuse the terminal's bar (`TerminalView::render_find_bar`) by moving it into a shared component. This is the recommended option.
  2. Inject the bar into the page itself (shadow DOM). This floats like cmux's, but pages can break or restyle it, and it's lost on navigation.
  3. Put the bar in a native child `NSView` above the webview. This is the most faithful copy, but it means AppKit UI code outside gpui.

**Keys.** These go in the `Browser` key context, which needs adding to `BrowserView::render`, so they don't clash with the terminal's bindings:
- ⌘F opens the bar.
- ⌘G / ⇧⌘G (and Return / Shift+Return in the field) go to the next and previous match.
- Esc closes the bar and runs `clear()`.
- ⌘E searches for `window.getSelection().toString()`.

While the page has focus, keystrokes go to the webview and not to gpui ("Shortcuts while a webview has focus" in `missing-cmux-features.md`). So ⌘F has to work the way ⌘A/C/V/X do: the Edit menu, or a key monitor like `watch_clicks`.

**Lifecycle.**
- After a navigation, the marks are gone and the stored array is stale. On `WebMsg::Url`, re-run `search` with the current needle if the bar is open, or reset the count.
- Like cmux, `next` and `previous` already recover when the page has dropped a mark.

## Known limits of this approach (cmux's too)

- **It changes the page's DOM.** Wrapping text in `<mark>` can upset framework-rendered pages such as React, and `clear()` only puts back text nodes, not the original node identity.
- **Matches never span element boundaries:** "foo **bar**" won't match "foo bar".
- **It doesn't reach** iframes, shadow DOM or closed `<details>` content.
- **Case folding** is JS `toLowerCase`, so it isn't diacritic-insensitive.

The alternative that avoids all of this is WebKit's own `WKWebView.find(_:configuration:completionHandler:)` (`WKFindConfiguration`, macOS 11+). It highlights natively without touching the DOM, but it only reports whether there was a match, not a count. That means no `k/N` in the bar. A mix is possible: native `find` for moving between matches, plus a read-only JS pass that counts without wrapping anything.
