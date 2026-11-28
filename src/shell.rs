//! Shell integration: abromux makes zsh and bash load a script of its own
//! after the user's startup files, which adds OSC 133 prompt marks. With the
//! marks, libghostty-vt knows the prompt rows and blanks them on resize for
//! the shell to redraw instead of reflowing them, which otherwise leaves
//! copies of the prompt line behind. The scripts also wrap `codex` to run
//! without Codex's shared background server, so Codex's hooks report to the
//! terminal they run in (`agent.rs`).
//!
//! - zsh starts with `ZDOTDIR` pointing at abromux's directory, whose
//!   `.zshenv` restores the user's `ZDOTDIR` and runs their `.zshenv`
//!   (`src/shell/zsh`).
//! - bash starts as `bash --posix -l` with `ENV` pointing at abromux's
//!   script: in POSIX mode bash reads only `$ENV`, and the script leaves
//!   POSIX mode and reads the usual startup files (`src/shell/bash`), as
//!   Ghostty's does.
//!
//! Off with `shell-integration = none` in the Ghostty config.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const ZSH: &[(&str, &str)] = &[
    (".zshenv", include_str!("shell/zsh/.zshenv")),
    ("abromux.zsh", include_str!("shell/zsh/abromux.zsh")),
];

const BASH: &[(&str, &str)] = &[("abromux.bash", include_str!("shell/bash/abromux.bash"))];

/// Make the shell in `argv` (`[shell, "-l", ...]`) load the integration, if
/// abromux has one for it: the variables to set after `env`, and `argv`
/// changed if the shell needs other arguments for it.
pub fn inject(argv: &mut Vec<String>, env: &[(String, String)]) -> Vec<(String, String)> {
    let Some(shell) = argv.first() else {
        return vec![];
    };
    match Path::new(shell).file_name().and_then(|n| n.to_str()) {
        Some("zsh") => inject_zsh(env),
        Some("bash") => inject_bash(argv, env),
        _ => vec![],
    }
}

fn inject_zsh(env: &[(String, String)]) -> Vec<(String, String)> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    let Some(dir) = dir("zsh", ZSH, &DIR) else {
        return vec![];
    };
    // The pane's env wins over abromux's own, as it does for the shell.
    let zdotdir = env
        .iter()
        .rev()
        .find(|(k, _)| k == "ZDOTDIR")
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var("ZDOTDIR").ok());
    zdotdir
        .map(|z| ("ABROMUX_ZSH_ZDOTDIR".to_string(), z))
        .into_iter()
        .chain([("ZDOTDIR".into(), dir.to_string_lossy().into_owned())])
        .collect()
}

/// Whether the bash at `path` is 4 or later. An older one (macOS's
/// `/bin/bash` is 3.2) doesn't read `$ENV` as an interactive login shell in
/// POSIX mode, so the integration would never load and the shell would stay
/// in POSIX mode (Ghostty skips it too). Asked once per path.
fn bash_takes_integration(path: &str) -> bool {
    static SEEN: std::sync::Mutex<Vec<(String, bool)>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, ok)) = seen.iter().find(|(p, _)| p == path) {
        return *ok;
    }
    let ok = bash_major(path).is_some_and(|v| v >= 4);
    seen.push((path.to_string(), ok));
    ok
}

/// The major version of the bash at `path`, from `BASH_VERSINFO`.
fn bash_major(path: &str) -> Option<u32> {
    let out = std::process::Command::new(path)
        .args([
            "--norc",
            "--noprofile",
            "-c",
            "echo \"${BASH_VERSINFO[0]}\"",
        ])
        .env_remove("BASH_ENV")
        .env_remove("ENV")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Only an interactive login shell (`bash -l`, as terminals start it); one
/// running a command (`-c`) needs no prompt marks.
fn inject_bash(argv: &mut Vec<String>, env: &[(String, String)]) -> Vec<(String, String)> {
    if argv.len() != 2 || argv[1] != "-l" || !bash_takes_integration(&argv[0]) {
        return vec![];
    }
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    let Some(dir) = dir("bash", BASH, &DIR) else {
        return vec![];
    };
    // The pane's env wins over abromux's own, as it does for the shell.
    let var = |name: &str| {
        env.iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var(name).ok())
    };
    argv.splice(1.., ["--posix".to_string(), "-l".to_string()]);
    let mut vars = vec![
        ("ABROMUX_BASH_INJECT".to_string(), "1".to_string()),
        (
            "ENV".into(),
            dir.join("abromux.bash").to_string_lossy().into_owned(),
        ),
    ];
    if let Some(e) = var("ENV") {
        vars.push(("ABROMUX_BASH_ENV".into(), e));
    }
    // POSIX mode would read and write ~/.sh_history.
    if var("HISTFILE").is_none() {
        let home = abromux_core::paths::home().join(".bash_history");
        vars.push(("HISTFILE".into(), home.to_string_lossy().into_owned()));
        vars.push(("ABROMUX_BASH_UNEXPORT_HISTFILE".into(), "1".into()));
    }
    vars
}

/// A shell's integration files under the support folder, written once per
/// run (into `cell`) so they match this build.
fn dir(
    shell: &str,
    files: &[(&str, &str)],
    cell: &'static OnceLock<Option<PathBuf>>,
) -> Option<&'static Path> {
    cell.get_or_init(|| {
        let dir = crate::channel::support_dir()
            .join("shell-integration")
            .join(shell);
        write_files(&dir, files)
            .inspect_err(|e| eprintln!("abromux: {shell} integration: {e}"))
            .ok()?;
        Some(dir)
    })
    .as_deref()
}

/// Write the command to resume in a new shell where its integration reads
/// it (`_abromux_resume`, which removes it): a file only this user can
/// read, named by a fresh random token.
pub fn resume_file(r: &abromux_core::program_resume::Restart) -> std::io::Result<PathBuf> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
    let dir = crate::channel::support_dir().join("resume");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    // Ones a shell never got to (it was closed before its first prompt).
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(|| {
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > day))
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    });
    let path = dir.join(abromux_core::program_resume::new_token());
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(&r.file_bytes())?;
    Ok(path)
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
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Real zsh in a real pty, its output parsed by libghostty-vt, while the
    /// pane is squeezed down to 2 columns and widened again. Returns the
    /// screen after each step.
    fn squeeze(shell: &str, integration: bool) -> Vec<String> {
        let home = std::env::temp_dir().join(format!(
            "abromux-{shell}-test-{}-{integration}",
            std::process::id()
        ));
        let dir = home.join("integration");
        write_files(&dir, if shell == "zsh" { ZSH } else { BASH }).unwrap();
        let (rc, prompt) = match shell {
            "zsh" => (".zshrc", "PS1='zilvinas@Zilvinas-MacBook-Pro ~ %# '\n"),
            _ => (
                ".bash_profile",
                "PS1='zilvinas@Zilvinas-MacBook-Pro ~ $ '\n",
            ),
        };
        std::fs::write(home.join(rc), prompt).unwrap();

        let size = |cols| PtySize {
            rows: 12,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pty = native_pty_system().openpty(size(80)).unwrap();
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("HOME", &home);
        cmd.env("TERM", "xterm-256color");
        match (shell, integration) {
            ("zsh", true) => cmd.env("ZDOTDIR", &dir),
            ("bash", true) => {
                cmd.arg("--posix");
                cmd.env("ENV", dir.join("abromux.bash"));
                cmd.env("ABROMUX_BASH_INJECT", "1");
            }
            _ => {}
        }
        cmd.arg("-l");
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
        let mut term = Terminal::new(80, 12, crate::theme::is_dark()).unwrap();
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

    /// The arguments a fake `codex` gets when `line` runs in an interactive
    /// zsh with the integration. The fake knows `--no-daemon` if `new`.
    fn codex_args(line: &str, new: bool) -> String {
        let home =
            std::env::temp_dir().join(format!("abromux-codex-test-{}-{new}", std::process::id()));
        let dir = home.join("integration");
        write_files(&dir, ZSH).unwrap();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let help = if new { "--no-daemon" } else { "--remote" };
        let fake = bin.join("codex");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\n[ \"$1\" = --help ] && {{ echo '{help}'; exit; }}\necho \"$@\"\n"),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let out = std::process::Command::new("zsh")
            .args(["-i", "-c", line])
            .env("HOME", &home)
            .env("ZDOTDIR", &dir)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&home);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    #[ignore = "runs a real zsh"]
    fn zsh_runs_codex_without_its_daemon() {
        assert_eq!(codex_args("codex", true), "--no-daemon");
        assert_eq!(
            codex_args("codex resume abc", true),
            "--no-daemon resume abc"
        );
        assert_eq!(codex_args("codex agents", true), "agents");
        // A Codex without the flag gets the command unchanged.
        assert_eq!(codex_args("codex resume abc", false), "resume abc");
    }

    #[test]
    #[ignore = "runs a real zsh"]
    fn zsh_prompt_survives_squeezing() {
        let prompts = |s: &String| s.matches("zilvinas@Zilvinas-MacBook-Pro").count();
        let without: Vec<_> = squeeze("zsh", false).iter().map(prompts).collect();
        let with: Vec<_> = squeeze("zsh", true).iter().map(prompts).collect();
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

    /// False (and says so) when the `bash` on PATH is too old for the
    /// integration, which abromux then doesn't load (macOS's 3.2).
    fn bash_new_enough() -> bool {
        let ok = super::bash_takes_integration("bash");
        if !ok {
            eprintln!("skipped: bash on PATH is older than 4");
        }
        ok
    }

    #[test]
    fn asks_bash_its_version() {
        assert_eq!(super::bash_major("/no/such/bash"), None);
        assert!(super::bash_major("bash").is_some_and(|v| v >= 3));
    }

    /// What a login `shell` with the integration prints, raw, when started
    /// with a resume file for `argv`: it should run it at the first prompt,
    /// after the startup files (which put the fake program on PATH), and
    /// remove the file.
    fn resumed(shell: &str, argv: &[&str]) -> (String, bool) {
        let home = std::env::temp_dir().join(format!(
            "abromux-resume-test-{shell}-{}",
            std::process::id()
        ));
        let dir = home.join("integration");
        write_files(&dir, if shell == "zsh" { ZSH } else { BASH }).unwrap();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("show");
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf 'in=%s\\n' \"$PWD\"; for a; do printf '[%s]\\n' \"$a\"; done\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let path = format!(
            "export PATH={}:$PATH\nshow() {{ echo FUNCTION; }}\n",
            bin.display()
        );
        let rc = if shell == "zsh" {
            ".zshrc"
        } else {
            ".bash_profile"
        };
        std::fs::write(home.join(rc), path).unwrap();
        let work = home.join("a dir");
        std::fs::create_dir_all(&work).unwrap();
        let file = home.join("resume-file");
        let r = abromux_core::program_resume::Restart {
            argv: argv.iter().map(|a| a.to_string()).collect(),
            cwd: Some(work.to_string_lossy().into_owned()),
            cwd_required: true,
        };
        std::fs::write(&file, r.file_bytes()).unwrap();

        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("HOME", &home);
        cmd.env("TERM", "xterm-256color");
        cmd.env("ABROMUX_RESUME_FILE", &file);
        if shell == "zsh" {
            cmd.env("ZDOTDIR", &dir);
        } else {
            cmd.arg("--posix");
            cmd.env("ENV", dir.join("abromux.bash"));
            cmd.env("ABROMUX_BASH_INJECT", "1");
        }
        cmd.arg("-l");
        let mut child = pty.slave.spawn_command(cmd).unwrap();
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut out = vec![];
        let end = Instant::now() + Duration::from_millis(1500);
        while let Ok(b) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            out.extend(b);
        }
        let _ = child.kill();
        let left = file.exists();
        let _ = std::fs::remove_dir_all(&home);
        (String::from_utf8_lossy(&out).into_owned(), left)
    }

    #[test]
    #[ignore = "runs a real zsh and bash"]
    fn shells_resume_the_command_at_the_first_prompt() {
        for shell in ["zsh", "bash"] {
            if shell == "bash" && !bash_new_enough() {
                continue;
            }
            let (out, left) = resumed(shell, &["show", "", "a b", "$(echo no)", ";x"]);
            let out = out.replace("\r\n", "\n");
            assert!(!left, "{shell} left the file");
            assert!(!out.contains("FUNCTION"), "{shell}: {out}");
            assert!(
                out.contains("a dir\n[]\n[a b]\n[$(echo no)]\n[;x]\n"),
                "{shell}: {out}"
            );
            // Marked as a command, so the app knows when it ends.
            let c = out.find("\x1b]133;C\x07").expect("C mark");
            let d = out.find("\x1b]133;D;0\x07").expect("D mark");
            assert!(c < out.find("[a b]").unwrap() && out.find("[a b]").unwrap() < d);
        }
    }

    /// What an interactive bash with the integration prints for `line`, raw
    /// (with the OSC 133 marks), run as abromux runs it: `bash --posix -l`
    /// with ENV, in a pty, `HOME` holding `files`.
    fn bash_output(files: &[(&str, &str)], line: &str) -> String {
        let home = std::env::temp_dir().join(format!(
            "abromux-bash-test-{}-{}",
            std::process::id(),
            line.len()
        ));
        let dir = home.join("integration");
        write_files(&dir, BASH).unwrap();
        write_files(&home, files).unwrap();
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new("bash");
        cmd.args(["--posix", "-l"]);
        cmd.env("HOME", &home);
        cmd.env("TERM", "xterm-256color");
        cmd.env("ENV", dir.join("abromux.bash"));
        cmd.env("ABROMUX_BASH_INJECT", "1");
        cmd.env("HISTFILE", home.join(".bash_history"));
        cmd.env("ABROMUX_BASH_UNEXPORT_HISTFILE", "1");
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
        let mut out = vec![];
        let pump = |out: &mut Vec<u8>, ms| {
            let end = Instant::now() + Duration::from_millis(ms);
            while let Ok(b) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                out.extend(b);
            }
        };
        pump(&mut out, 1000);
        writer.write_all(format!("{line}\r").as_bytes()).unwrap();
        pump(&mut out, 1000);
        let _ = child.kill();
        let _ = std::fs::remove_dir_all(&home);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    #[ignore = "runs a real bash"]
    fn bash_reads_startup_files_and_marks_prompts() {
        if !bash_new_enough() {
            return;
        }
        let out = bash_output(
            &[
                (".bash_profile", "export FROM=profile; PS1='p> '\n"),
                (".bashrc", "export FROM=bashrc\n"),
            ],
            "echo \"from=$FROM\"; shopt -q login_shell && echo login; \
             [[ -o posix ]] || echo noposix; echo \"inject=${ABROMUX_BASH_INJECT-unset}\"; \
             echo \"hist=${HISTFILE##*/}\"; false",
        );
        // A login shell reads ~/.bash_profile (which here doesn't source
        // ~/.bashrc), out of POSIX mode, with bash's own history file.
        for want in [
            "from=profile",
            "login",
            "noposix",
            "inject=unset",
            "hist=.bash_history",
        ] {
            assert!(out.contains(want), "{want} missing in {out:?}");
        }
        // Prompt start and end, output start, and the command's status.
        for mark in [
            "\x1b]133;A;redraw=1\x07p> \x1b]133;B\x07",
            "\x1b]133;C\x07",
            "\x1b]133;D;1\x07",
        ] {
            assert!(out.contains(mark), "{mark:?} missing in {out:?}");
        }
    }

    #[test]
    #[ignore = "runs a real bash"]
    fn bash_runs_codex_without_its_daemon() {
        if !bash_new_enough() {
            return;
        }
        let home = std::env::temp_dir().join(format!("abromux-bash-codex-{}", std::process::id()));
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("codex");
        std::fs::write(
            &fake,
            "#!/bin/sh\n[ \"$1\" = --help ] && { echo '--no-daemon'; exit; }\necho \"args: $@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let out = bash_output(
            &[(".bash_profile", &format!("export PATH={path}\n"))],
            "codex resume abc; codex agents",
        );
        let _ = std::fs::remove_dir_all(&home);
        assert!(out.contains("args: --no-daemon resume abc"), "{out:?}");
        assert!(out.contains("args: agents"), "{out:?}");
    }

    #[test]
    fn bash_commands_get_no_integration() {
        let mut argv = vec![
            "/bin/bash".to_string(),
            "-l".into(),
            "-c".into(),
            "ls".into(),
        ];
        assert!(inject(&mut argv, &[]).is_empty());
        assert_eq!(argv, ["/bin/bash", "-l", "-c", "ls"]);
        let mut argv = vec!["/bin/fish".to_string(), "-l".into()];
        assert!(inject(&mut argv, &[]).is_empty());
    }

    #[test]
    #[ignore = "runs a real bash"]
    fn bash_prompt_survives_squeezing() {
        if !bash_new_enough() {
            return;
        }
        let prompts = |s: &String| s.matches("zilvinas@Zilvinas-MacBook-Pro").count();
        let without: Vec<_> = squeeze("bash", false).iter().map(prompts).collect();
        let with: Vec<_> = squeeze("bash", true).iter().map(prompts).collect();
        eprintln!("prompts on screen without integration: {without:?}");
        eprintln!("prompts on screen with integration:    {with:?}");
        // As with zsh: readline redraws the prompt the terminal blanked.
        assert!(without.iter().max() > Some(&2));
        assert!(with.iter().all(|&n| n <= 2), "{with:?}");
        assert_eq!(with.last(), Some(&1));
    }
}
