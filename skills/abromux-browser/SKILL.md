---
name: abromux-browser
description: "Open and drive a browser pane inside AbroMux, beside the agent's terminal: load a page, read it as an accessibility tree, click, fill forms, wait, screenshot, read its console, errors and network requests, work inside iframes, handle cookies, storage and downloads. Use when running inside abromux ($ABROMUX_SURFACE_ID is set) and the user wants you to check a web page or local dev server, test a UI flow, or show them a page next to the conversation. For terminals, splits and sidebar status, use the abromux skill."
---

# AbroMux browser

AbroMux browser panes are native WebKit views next to the terminals. `abromux browser` opens them and automates the page, so the user watches what you do.

## Check first

```bash
test -n "${ABROMUX_SURFACE_ID:-}" && abromux identify
```

If `ABROMUX_SURFACE_ID` is empty, you aren't inside abromux: say so and stop.

`abromux --help` (the `browser` section) is the authority on subcommands and flags.

## Open a page

```bash
abromux browser open http://localhost:3000 --no-focus
# OK surface:8 pane:6 workspace:1
abromux browser --surface surface:8 wait --load complete
```

`open` splits to the right of your terminal (`--direction left|right|up|down`). `--no-focus` keeps the keyboard in your terminal; drop it only if the user wants to use the page. Keep the surface ref and pass `--surface` on every later command. Without it, commands go to the workspace's focused browser, else its first one, which may not be yours.

```bash
abromux browser --surface surface:8 goto https://example.com/login
abromux browser --surface surface:8 back      # also forward, reload
abromux browser --surface surface:8 url
```

## The loop: snapshot, act, check

1. **Snapshot** to see the page as an accessibility tree. Each element gets a ref:

   ```bash
   abromux browser --surface surface:8 snapshot --interactive
   # - textbox "Email" [ref=e2]
   # - textbox "Password" [ref=e3]
   # - button "Sign in" [ref=e4]
   ```

   `--interactive` lists only things you can act on (start with it); `--compact` trims structure; `--selector <css>` limits it to part of the page.

2. **Act** with a ref (`e2`) or a CSS selector:

   ```bash
   abromux browser --surface surface:8 fill e2 'alice@example.com'
   abromux browser --surface surface:8 fill e3 'hunter2'
   abromux browser --surface surface:8 click e4
   ```

3. **Wait, then check.** Pages change after actions; wait for the result before you read it:

   ```bash
   abromux browser --surface surface:8 wait --text 'Welcome' --timeout 5000
   abromux browser --surface surface:8 get text '#result'
   ```

Refs come from the latest snapshot. After navigation or a big DOM change, snapshot again before using refs.

## Commands

| Need | Command |
|---|---|
| Click, hover, focus | `click`, `dblclick`, `hover`, `focus`, `scroll-into-view` `<sel>` |
| Text fields | `fill <sel> <text>` replaces the value; `type <sel> <text>` types key by key after what's there |
| Keys | `press Enter` (also `Tab`, `Escape`, `ArrowDown`, `ctrl+a`; `--selector` to target a field); `keydown Shift` / `keyup Shift` send just that event, to hold a key |
| Checkboxes, selects | `check`/`uncheck <sel>`, `select <sel> <value>...` |
| Scroll | `scroll down 800` (`up`/`left`/`right`; `--selector` for a scrollable element) |
| Read | `get text\|html\|value\|title\|url\|count\|box <sel>`, `get attr <sel> <name>`, `get styles <sel> <names>...` |
| Test state | `is visible\|hidden\|enabled\|disabled\|checked\|editable\|focused <sel>` (prints true/false) |
| Find without a snapshot | `find role button --name 'Sign in'`, `find text\|label\|placeholder\|alt\|title\|testid <value>` (`--exact`) |
| Wait | `wait <sel>` (`--state visible\|hidden\|attached\|detached`), `wait --text`, `--url <part>`, `--load interactive\|complete`, `--js <expr>`; `--timeout <ms>` (default 10000) |
| Zoom | `zoom 125%` (or `in`, `out`, `reset`, `1.5`; no argument prints it) |
| Debug the page | `console` (`--level error`), `errors` (uncaught exceptions and rejections), `network` (`--filter api`): what the page logged, threw and requested since the pane opened; `--clear` empties the list after printing, `--limit 20` keeps the newest |
| Iframes | `frame e7` (a ref or selector of an `<iframe>`, cross-origin ones too) runs every later command in it; `frame main` goes back; `frame` says which is picked. A navigation of the page goes back to main by itself |
| Tabs | `tab` lists the browser tabs in the pane, `tab new <url>`, `tab switch 1`, `tab close` |
| Cookies | `cookies` (the page's, HttpOnly included; `--all` for every site), `cookies set <name> <value>` (`--domain`, `--path`, `--expires`, `--secure`, `--http-only`, `--same-site`), `cookies clear [<name>]` |
| Storage | `storage local` (or `session`) prints it all; `storage local get\|set\|remove <key> [<value>]`, `storage local clear` |
| Sessions | `state save auth.json` writes cookies and the page's localStorage (Playwright's storageState format); `state load auth.json` restores them (localStorage only for the page's own origin, so open it first) |
| Downloads | click the link, then `wait-download --path ./out/` (waits for the running download, else the next one; prints the file); `downloads` lists this session's |
| Anything else | `eval '<js>'` prints the result |
| See it | `screenshot shot.png` (no path: a file in `$TMPDIR`; prints the path). Read the PNG to look at it. |

## Limits

- `console`, `errors` and `network` see what happened after the pane opened, including in iframes; `network` lists fetch/XHR with their status and the page's other loads (from the Resource Timing API, whose status may be blank).
- Events are synthesized in the page with JavaScript. Sites that only react to trusted input (some drag-and-drop, some rich editors, file pickers) won't respond. Say so rather than retrying.
- The pane keeps cookies between sessions, so it may be signed in to sites. Don't submit forms that buy, send, delete or publish anything without asking.
- When done, close the browser if you opened it only for your own checks: `abromux close-surface --surface surface:8`. Leave it if the user wants to keep looking.
