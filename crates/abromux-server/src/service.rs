//! `abromux-server install` / `uninstall`: run the server as a systemd user
//! service, so it's up from boot (with lingering) rather than started by the
//! first ssh connection, and doesn't exit when idle.

use anyhow::{Context as _, bail};
use std::path::PathBuf;
use std::process::Command;

const UNIT: &str = "abromux-server.service";

fn unit_path() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| abromux_core::paths::home().join(".config"));
    Ok(base.join("systemd/user").join(UNIT))
}

/// The unit's text, for the binary at `exe` serving `socket`, with `extra`
/// flags (`--tailnet`, ...).
pub fn unit(exe: &str, socket: &str, extra: &[String]) -> String {
    let extra: String = extra.iter().map(|a| format!(" \"{a}\"")).collect();
    format!(
        "[Unit]\n\
         Description=abromux server: keeps abromux terminals running\n\
         Documentation=https://github.com/ZilvinasAbr/abromux/blob/main/docs/server-plan.md\n\
         \n\
         [Service]\n\
         ExecStart=\"{exe}\" serve --socket \"{socket}\" --no-idle-exit{extra}\n\
         Restart=on-failure\n\
         # Its terminals are its children: stopping it ends them.\n\
         KillMode=control-group\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

fn systemctl(args: &[&str]) -> anyhow::Result<()> {
    let st = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .context("can't run systemctl")?;
    if !st.success() {
        bail!("systemctl --user {}: {st}", args.join(" "));
    }
    Ok(())
}

/// Write the unit, enable and start it. `--no-start` only writes it; other
/// flags go to the server (`--tailnet`, `--tailnet-allow ..`).
pub fn install(args: &[String]) -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        bail!("on a Mac the app starts the server itself; this is for Linux (systemd)");
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    let path = unit_path()?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    let socket = crate::default_socket();
    std::fs::write(
        &path,
        unit(
            &exe.to_string_lossy(),
            &socket.to_string_lossy(),
            &args
                .iter()
                .filter(|a| *a != "--no-start")
                .cloned()
                .collect::<Vec<_>>(),
        ),
    )?;
    println!("Wrote {}", path.display());
    if args.iter().any(|a| a == "--no-start") {
        return Ok(());
    }
    systemctl(&["daemon-reload"])?;
    // A server started on demand (by ssh) would keep the socket: stop it
    // first, which ends its terminals.
    if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
        println!(
            "A server is already running on {} (started on demand). It keeps running; \
             the service takes over once it exits (with its last terminal).",
            socket.display()
        );
    }
    systemctl(&["enable", "--now", UNIT])?;
    println!("Enabled and started {UNIT}.");
    let user = std::env::var("USER").unwrap_or_default();
    let linger = Command::new("loginctl")
        .args(["show-user", &user, "-p", "Linger", "--value"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes")
        .unwrap_or(false);
    if !linger {
        println!(
            "To keep it (and its terminals) running while you're logged out and \
             start it at boot: loginctl enable-linger {user}"
        );
    }
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let path = unit_path()?;
    let _ = systemctl(&["disable", "--now", UNIT]);
    if path.exists() {
        std::fs::remove_file(&path)?;
        println!("Removed {}", path.display());
    }
    let _ = systemctl(&["daemon-reload"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn unit_runs_the_server_without_idle_exit() {
        let u = super::unit(
            "/home/me/.local/bin/abromux-server",
            "/home/me/.local/state/abromux/server.sock",
            &["--tailnet".into()],
        );
        assert!(u.contains("--no-idle-exit \"--tailnet\""), "{u}");
        assert!(u.contains("ExecStart=\"/home/me/.local/bin/abromux-server\" serve --socket"));
        assert!(u.contains("--no-idle-exit"));
        assert!(u.contains("WantedBy=default.target"));
    }
}
