//! The `tailscale` CLI, for the phone bridge: who and where this machine is
//! on the tailnet, and `tailscale serve`, which puts the bridge's loopback
//! port on the tailnet over HTTPS and tells it who's asking
//! (`Tailscale-User-Login`).

use anyhow::{Context as _, bail};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

/// The `tailscale` binary: on `$PATH`, else where its installers put it.
pub fn binary() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("tailscale"))
            .collect::<Vec<_>>()
    });
    on_path
        .chain(
            [
                "/usr/local/bin/tailscale",
                "/opt/homebrew/bin/tailscale",
                "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
                "/usr/bin/tailscale",
            ]
            .map(PathBuf::from),
        )
        .find(|p| p.is_file())
}

fn run(args: &[&str]) -> anyhow::Result<String> {
    let bin = binary().context("tailscale isn't installed (https://tailscale.com/download)")?;
    let out = Command::new(&bin)
        .args(args)
        .output()
        .with_context(|| format!("running {}", bin.display()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        bail!(
            "tailscale {}: {}",
            args.join(" "),
            if err.is_empty() { "failed" } else { err }
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// This machine on the tailnet.
pub struct Me {
    /// Its MagicDNS name, `mac.tail1234.ts.net`.
    pub host: String,
    /// The login of the user who owns it, `name@example.com`.
    pub login: String,
}

pub fn me() -> anyhow::Result<Me> {
    let status: Value = serde_json::from_str(&run(&["status", "--json"])?)
        .context("tailscale status --json: not JSON")?;
    parse_me(&status)
}

fn parse_me(status: &Value) -> anyhow::Result<Me> {
    let state = status["BackendState"].as_str().unwrap_or_default();
    if state != "Running" {
        bail!("tailscale isn't connected ({state}); run `tailscale up`");
    }
    let me = &status["Self"];
    let host = me["DNSName"]
        .as_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_string();
    if host.is_empty() {
        bail!("this machine has no MagicDNS name; turn on MagicDNS for the tailnet");
    }
    let uid = me["UserID"].as_u64().unwrap_or_default().to_string();
    let login = status["User"][&uid]["LoginName"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if login.is_empty() {
        bail!("this machine has no owner on the tailnet (a tagged device?)");
    }
    Ok(Me { host, login })
}

/// Serve `http://127.0.0.1:<local>` on the tailnet at `https://<host>:<https>`,
/// in the background (it lasts until turned off, across restarts).
pub fn serve(https: u16, local: u16) -> anyhow::Result<()> {
    run(&[
        "serve",
        "--bg",
        &format!("--https={https}"),
        &format!("http://127.0.0.1:{local}"),
    ])
    .map(drop)
}

/// Stop serving on `https`.
pub fn unserve(https: u16) -> anyhow::Result<()> {
    run(&["serve", &format!("--https={https}"), "off"]).map(drop)
}

/// What `tailscale serve` proxies `https` to (`http://127.0.0.1:7298`), if anything.
pub fn serving(https: u16) -> anyhow::Result<Option<String>> {
    let out = run(&["serve", "status", "--json"])?;
    let status: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    Ok(parse_serving(&status, https))
}

fn parse_serving(status: &Value, https: u16) -> Option<String> {
    let web = status["Web"].as_object()?;
    let suffix = format!(":{https}");
    web.iter()
        .find(|(host, _)| host.ends_with(&suffix))
        .and_then(|(_, v)| v["Handlers"]["/"]["Proxy"].as_str())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_self_and_owner() {
        let status = json!({
            "BackendState": "Running",
            "Self": { "DNSName": "mac.tail1.ts.net.", "UserID": 42 },
            "User": { "42": { "LoginName": "me@example.com" } },
        });
        let me = parse_me(&status).unwrap();
        assert_eq!(me.host, "mac.tail1.ts.net");
        assert_eq!(me.login, "me@example.com");
        assert!(parse_me(&json!({ "BackendState": "Stopped" })).is_err());
    }

    #[test]
    fn finds_what_a_port_serves() {
        let status = json!({
            "TCP": { "443": { "HTTPS": true }, "8443": { "HTTPS": true } },
            "Web": {
                "mac.tail1.ts.net:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:8010" } } },
                "mac.tail1.ts.net:8443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:7298" } } },
            },
        });
        assert_eq!(
            parse_serving(&status, 8443).as_deref(),
            Some("http://127.0.0.1:7298")
        );
        assert_eq!(parse_serving(&status, 10000), None);
    }
}
