# AbroMux

A terminal and browser workspace app for macOS, written fully in Rust with [GPUI](https://gpui.rs) and [libghostty](https://github.com/ghostty-org/ghostty).

## Why

I love [cmux](https://cmux.com) and use it every day. But with many agents and browser tabs open at once, it started to feel slow. I wanted to see whether a leaner, faster take on the same idea was possible, built fully in Rust. AbroMux is that experiment.

## What it does

- **Workspaces** in a sidebar, each with its git branch, cwd and status, a description and a color, in groups you can fold and drag to reorder.
- **Split panes** with tabs of terminals (libghostty) and browsers (native WebKit).
- **Agent aware**: Claude Code, Codex, Gemini CLI, Droid, Cursor, OpenCode and Pi show when they're running or need input, notify you, and resume after a restart.
- **Session restore**: shells keep running while abromux is closed and come back as they were; after a reboot, layout, scrollback and agent sessions come back.
- **Scriptable** through the `abromux` CLI, with commands named after cmux's.
- **Reads your Ghostty config** for theme, font and colors.

## Install

Download `AbroMux-<version>.dmg` from the [latest release](https://github.com/ZilvinasAbr/abromux/releases/latest), open it and drag AbroMux to Applications. It needs macOS 13 or later on Apple Silicon.

Optionally, put the `abromux` CLI on your PATH, turn on coding-agent integration, and teach those agents the CLI with skills:

```bash
sudo ln -sf /Applications/AbroMux.app/Contents/MacOS/abromux /usr/local/bin/abromux
abromux hooks install
abromux skills install
```

## Learn more

[docs/guide.md](docs/guide.md) has the full feature list, keyboard shortcuts, the CLI reference, and how to build AbroMux from source.

## License

[MIT](LICENSE)
