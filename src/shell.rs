//! Shell integration: abromux starts zsh with `ZDOTDIR` pointing at a
//! directory of its own, whose `.zshenv` restores the user's `ZDOTDIR`, runs
//! their `.zshenv` and adds OSC 133 prompt marks (`src/shell/zsh`). With the
//! marks, libghostty-vt blanks the prompt on resize for zsh to redraw instead
//! of reflowing it, which otherwise leaves copies of the prompt line behind.
//! Off with `shell-integration = none` in the Ghostty config.

use portable_pty::CommandBuilder;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const ZSH: &[(&str, &str)] = &[
    (".zshenv", include_str!("shell/zsh/.zshenv")),
    ("abromux.zsh", include_str!("shell/zsh/abromux.zsh")),
];

/// Set up `cmd` (which runs `shell`) to load the integration, if abromux
/// has one for that shell.
pub fn inject(cmd: &mut CommandBuilder, shell: &str, env: &[(String, String)]) {
    if Path::new(shell).file_name().and_then(|n| n.to_str()) != Some("zsh") {
        return;
    }
    let Some(dir) = zsh_dir() else { return };
    // The pane's env wins over abromux's own, as it does for the shell.
    let zdotdir = env
        .iter()
        .rev()
        .find(|(k, _)| k == "ZDOTDIR")
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var("ZDOTDIR").ok());
    if let Some(z) = zdotdir {
        cmd.env("ABROMUX_ZSH_ZDOTDIR", z);
    }
    cmd.env("ZDOTDIR", dir);
}

/// The zsh integration files, written once per run so they match this build.
fn zsh_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = crate::channel::support_dir().join("shell-integration/zsh");
        write_files(&dir, ZSH)
            .inspect_err(|e| eprintln!("abromux: zsh integration: {e}"))
            .ok()?;
        Some(dir)
    })
    .as_deref()
}

fn write_files(dir: &Path, files: &[(&str, &str)]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, text) in files {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
            std::fs::write(path, text)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghostty::Terminal;
    use portable_pty::{PtySize, native_pty_system};
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Real zsh in a real pty, its output parsed by libghostty-vt, while the
    /// pane is squeezed down to 2 columns and widened again. Returns the
    /// screen after each step.
    fn squeeze(integration: bool) -> Vec<String> {
        let home = std::env::temp_dir().join(format!(
            "abromux-zsh-test-{}-{integration}",
            std::process::id()
        ));
        let dir = home.join("integration");
        write_files(&dir, ZSH).unwrap();
        std::fs::write(
            home.join(".zshrc"),
            "PS1='zilvinas@Zilvinas-MacBook-Pro ~ %# '\n",
        )
        .unwrap();

        let size = |cols| PtySize {
            rows: 12,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pty = native_pty_system().openpty(size(80)).unwrap();
        let mut cmd = CommandBuilder::new("zsh");
        cmd.arg("-l");
        cmd.env("HOME", &home);
        cmd.env("TERM", "xterm-256color");
        if integration {
            cmd.env("ZDOTDIR", &dir);
        }
        let mut child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        let mut writer = pty.master.take_writer().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut term = Terminal::new(80, 12).unwrap();
        let pump = |term: &mut Terminal, ms| {
            let end = Instant::now() + Duration::from_millis(ms);
            while let Ok(b) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                term.write(&b);
            }
        };
        pump(&mut term, 1500);
        writer.write_all(b"echo zilvinas zilvinas").unwrap();
        pump(&mut term, 300);

        let mut screens = vec![];
        for cols in [60, 50, 45, 40, 30, 20, 10, 5, 3, 2, 10, 30, 80] {
            term.resize(cols, 12, 10, 20);
            pty.master.resize(size(cols)).unwrap();
            pump(&mut term, 300);
            screens.push(format!("{cols} cols:\n{}", term.screen_text().trim_end()));
        }
        let _ = child.kill();
        let _ = std::fs::remove_dir_all(&home);
        screens
    }

    #[test]
    #[ignore = "runs a real zsh"]
    fn zsh_prompt_survives_squeezing() {
        let prompts = |s: &String| s.matches("zilvinas@Zilvinas-MacBook-Pro").count();
        let without: Vec<_> = squeeze(false).iter().map(prompts).collect();
        let with: Vec<_> = squeeze(true).iter().map(prompts).collect();
        eprintln!("prompts on screen without integration: {without:?}");
        eprintln!("prompts on screen with integration:    {with:?}");
        // Without the marks every resize that wraps the prompt leaves a copy.
        assert!(without.iter().max() > Some(&2));
        // With them libghostty-vt clears the prompt for zsh to redraw. Since
        // ghostty-org/ghostty@dde3d4d6b it clears after the reflow, from the
        // last wrapped row only, so one copy can stay until the pane widens.
        assert!(with.iter().all(|&n| n <= 2), "{with:?}");
        assert_eq!(with.last(), Some(&1));
    }
}
