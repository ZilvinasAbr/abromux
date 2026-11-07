//! Personal access tokens, one per host, in the login Keychain (service
//! `abromux-gitlab`, account = the host URL). `ABROMUX_GITLAB_TOKEN`
//! overrides it, for development and tests.

use anyhow::{Context as _, bail};
use std::{
    io::Write as _,
    process::{Command, Stdio},
};

const SERVICE: &str = "abromux-gitlab";
const SECURITY: &str = "/usr/bin/security";

pub fn token(host: &str) -> Option<String> {
    if let Ok(t) = std::env::var("ABROMUX_GITLAB_TOKEN")
        && !t.is_empty()
    {
        return Some(t);
    }
    let out = Command::new(SECURITY)
        .args(["find-generic-password", "-s", SERVICE, "-a", host, "-w"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let t = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !t.is_empty()).then_some(t)
}

/// A host a token is kept for, so a new tab opens on the GitLab you signed
/// in to rather than the development one. With several, `security` picks one.
pub fn saved_host() -> Option<String> {
    let out = Command::new(SECURITY)
        .args(["find-generic-password", "-s", SERVICE])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| account(&String::from_utf8_lossy(&out.stdout)))?
}

/// The `acct` attribute in `security find-generic-password` output.
fn account(attrs: &str) -> Option<String> {
    let v = attrs.lines().find_map(|l| l.trim().strip_prefix("\"acct\"<blob>=\""))?;
    let v = v.strip_suffix('"')?;
    (!v.is_empty()).then(|| v.to_string())
}

/// Keep `token` for `host`. It goes to `security` on stdin, not in argv.
/// `ABROMUX_GITLAB_NO_KEYCHAIN` skips it (dev/gitlab/app.sh: test tokens
/// stay out of your Keychain).
pub fn save_token(host: &str, token: &str) -> anyhow::Result<()> {
    if std::env::var_os("ABROMUX_GITLAB_NO_KEYCHAIN").is_some() {
        return Ok(());
    }
    let bad = |s: &str| s.contains(['"', '\\', '\n', '\r']);
    if bad(host) || bad(token) {
        bail!("the host or token has characters a Keychain item can't take");
    }
    let mut child = Command::new(SECURITY)
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("running security")?;
    writeln!(
        child.stdin.take().context("security stdin")?,
        "add-generic-password -U -s {SERVICE} -l \"abromux GitLab token\" -a \"{host}\" -w \"{token}\""
    )?;
    let out = child.wait_with_output()?;
    let err = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || !err.trim().is_empty() {
        bail!("couldn't save the token in the Keychain: {}", err.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn account_from_attributes() {
        let out = "attributes:\n    0x00000007 <blob>=\"abromux-gitlab\"\n    \"acct\"<blob>=\"https://gl.example.com\"\n    \"svce\"<blob>=\"abromux-gitlab\"\n";
        assert_eq!(super::account(out).as_deref(), Some("https://gl.example.com"));
        assert_eq!(super::account("    \"acct\"<blob>=<NULL>\n"), None);
    }
}
