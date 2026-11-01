//! Updates, the way cmux does them. cmux runs Sparkle behind its own UI: a
//! pill in the titlebar that says what the updater is doing and opens a
//! popover with the details and buttons. This is the same thing without
//! Sparkle:
//!
//! - **Checks** look at the repository's latest GitHub release. One runs on
//!   its own a few seconds after launch when the last was a day or more ago,
//!   and then daily (cmux's `SUScheduledCheckInterval`), unless
//!   `app.automaticallyCheckForUpdates` is off. "Check for Updates…" (app
//!   menu, palette, Settings) checks now. A background check that finds
//!   nothing shows nothing; one you asked for says "No Updates Available".
//! - **Found** a newer version: the pill says "Update Available: 0.13.0".
//!   Its popover has the version, size and date, a release-notes link, and
//!   Skip (don't offer this version again, unless you check by hand), Later
//!   and Install and Relaunch. Nothing downloads until you choose
//!   (`SUAutomaticallyUpdate` is off in cmux).
//! - **Install and Relaunch** downloads the release's disk image (the pill
//!   shows the percentage), mounts it, checks the app inside is signed by the
//!   same Developer ID team as this one (or, for a build signed ad hoc, is
//!   notarized), copies it next to this app, swaps the two, and quits. A
//!   small helper waits for the quit and opens the new app. Shells keep
//!   running across it like any quit.
//!
//! Nightly builds come from `scripts/nightly.sh`, not releases, so they only
//! check. `ABROMUX_UPDATE_FROM=0.1.0` pretends to be that version, and
//! `ABROMUX_UPDATE_FEED` points the check at another releases API URL, to try
//! it out.

use crate::{channel, theme::*};
use gpui::{prelude::*, *};
use serde::{Deserialize, Serialize};
use std::{
    io::Write as _,
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

actions!(update, [CheckForUpdates, ApplyUpdate]);

const REPO: &str = env!("CARGO_PKG_REPOSITORY");
/// The release app's bundle id; an update must be this app.
const BUNDLE_ID: &str = "dev.abromux.app";
/// cmux's `SUScheduledCheckInterval`.
const INTERVAL: u64 = 86_400;
/// How long after launch the first automatic check waits.
const FIRST_CHECK: Duration = Duration::from_secs(10);
/// How long "No Updates Available" stays up.
const NOT_FOUND_SHOWN: Duration = Duration::from_secs(5);
const POPOVER_W: f32 = 300.;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// The release's page, for its notes.
    pub url: String,
    /// `published_at`, as GitHub gives it.
    pub published: String,
    /// The disk image's download URL and size.
    pub dmg: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub title: &'static str,
    pub message: String,
}

fn failure(title: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        title,
        message: message.into(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    /// `user`: asked for, so it shows.
    Checking {
        user: bool,
    },
    Available(Release),
    NotFound,
    Error(Failure),
    Downloading {
        release: Release,
        got: u64,
    },
    /// Mounting, verifying and copying the new app.
    Preparing(Release),
    /// Swapping the apps; it relaunches next.
    Installing(Release),
}

pub struct Updater {
    pub state: State,
    /// The popover.
    pub open: bool,
    /// Bumped by each check or install, so a cancelled one's results are dropped.
    generation: u64,
    download: Option<Arc<Mutex<Child>>>,
}

struct Global(Entity<Updater>);
impl gpui::Global for Global {}

pub fn init(cx: &mut App) {
    let updater = cx.new(|_| Updater {
        state: State::Idle,
        open: false,
        generation: 0,
        download: None,
    });
    cx.set_global(Global(updater));
    cx.on_action(|_: &CheckForUpdates, cx| check(true, cx));
    cx.on_action(|_: &ApplyUpdate, cx| apply(cx));
    cx.spawn(async move |cx| {
        Timer::after(FIRST_CHECK).await;
        loop {
            let _ = cx.update(|cx| {
                if automatic(cx) && due(&Saved::load()) && entity(cx).read(cx).state == State::Idle
                {
                    check(false, cx)
                }
            });
            Timer::after(Duration::from_secs(3600)).await;
        }
    })
    .detach();
}

/// "Apply Update (If Available)": install what was found, else check.
pub fn apply(cx: &mut App) {
    if matches!(entity(cx).read(cx).state, State::Available(_)) {
        install(cx)
    } else {
        check(true, cx)
    }
}

pub fn entity(cx: &App) -> Entity<Updater> {
    cx.global::<Global>().0.clone()
}

/// Whether the popover is up, so browser panes (drawn above gpui) hide.
pub fn popover_open(cx: &App) -> bool {
    let u = entity(cx).read(cx);
    u.open && visible(&u.state)
}

fn automatic(cx: &App) -> bool {
    cx.global::<crate::settings::Settings>()
        .app
        .automatically_check_for_updates
        && !channel::nightly()
        && bundle().is_some()
}

fn due(saved: &Saved) -> bool {
    now().saturating_sub(saved.last_check) >= INTERVAL
}

fn update(cx: &mut App, f: impl FnOnce(&mut Updater)) {
    entity(cx).update(cx, |u, cx| {
        f(u);
        cx.notify();
    });
}

/// Whether `generation` is still the current job.
fn current(generation: u64, cx: &App) -> bool {
    entity(cx).read(cx).generation == generation
}

/// Start a new job, dropping whatever ran before.
fn begin(state: State, cx: &mut App) -> u64 {
    entity(cx).update(cx, |u, cx| {
        u.generation += 1;
        u.state = state;
        if let Some(child) = u.download.take() {
            let _ = child.lock().map(|mut c| c.kill());
        }
        cx.notify();
        u.generation
    })
}

pub fn check(user: bool, cx: &mut App) {
    match &entity(cx).read(cx).state {
        // Already at it: a check by hand just shows what's going on.
        State::Checking { .. }
        | State::Downloading { .. }
        | State::Preparing(_)
        | State::Installing(_) => {
            if user {
                update(cx, |u| {
                    if let State::Checking { user } = &mut u.state {
                        *user = true;
                    } else {
                        u.open = true;
                    }
                });
            }
            return;
        }
        _ => {}
    }
    let generation = begin(State::Checking { user }, cx);
    update(cx, |u| u.open = false);
    log(&format!(
        "checking ({})",
        if user { "user" } else { "scheduled" }
    ));
    cx.spawn(async move |cx| {
        let result = cx.background_executor().spawn(async { fetch() }).await;
        let _ = cx.update(|cx| checked(generation, user, result, cx));
    })
    .detach();
}

fn checked(generation: u64, user: bool, result: Result<Release, Failure>, cx: &mut App) {
    if !current(generation, cx) {
        return;
    }
    let mut saved = Saved::load();
    saved.last_check = now();
    saved.save();
    let state = match result {
        Ok(r) if newer(&r.version, &current_version()) => {
            log(&format!("found {}", r.version));
            if user || saved.skipped.as_deref() != Some(r.version.as_str()) {
                State::Available(r)
            } else {
                State::Idle
            }
        }
        Ok(r) => {
            log(&format!("up to date (latest {})", r.version));
            if user { State::NotFound } else { State::Idle }
        }
        Err(f) => {
            log(&format!("check failed: {}: {}", f.title, f.message));
            if user { State::Error(f) } else { State::Idle }
        }
    };
    let found = matches!(state, State::Available(_));
    let not_found = state == State::NotFound;
    update(cx, |u| {
        u.state = state;
        u.open = user && found;
    });
    if not_found {
        cx.spawn(async move |cx| {
            Timer::after(NOT_FOUND_SHOWN).await;
            let _ = cx.update(|cx| {
                if current(generation, cx) && entity(cx).read(cx).state == State::NotFound {
                    dismiss(cx)
                }
            });
        })
        .detach();
    }
}

/// Back to idle, the popover closed. Cancels a check or a download.
pub fn dismiss(cx: &mut App) {
    begin(State::Idle, cx);
    update(cx, |u| u.open = false);
}

fn skip(cx: &mut App) {
    if let State::Available(r) = &entity(cx).read(cx).state {
        let mut saved = Saved::load();
        saved.skipped = Some(r.version.clone());
        saved.save();
        log(&format!("skipped {}", r.version));
    }
    dismiss(cx);
}

pub fn install(cx: &mut App) {
    let State::Available(release) = entity(cx).read(cx).state.clone() else {
        return;
    };
    let app = bundle();
    if let Some(f) = location_problem(app.as_deref()) {
        log(&format!("can't install: {}", f.message));
        begin(State::Error(f), cx);
        update(cx, |u| u.open = true);
        return;
    }
    let app = app.expect("checked above");
    let generation = begin(
        State::Downloading {
            release: release.clone(),
            got: 0,
        },
        cx,
    );
    log(&format!("downloading {}", release.dmg));
    cx.spawn(async move |cx| {
        if let Err(f) = run_install(generation, release, app, cx).await {
            log(&format!("install failed: {}: {}", f.title, f.message));
            let _ = cx.update(|cx| {
                if current(generation, cx) {
                    begin(State::Error(f), cx);
                    update(cx, |u| u.open = true);
                }
            });
        }
    })
    .detach();
}

async fn run_install(
    generation: u64,
    release: Release,
    app: PathBuf,
    cx: &mut AsyncApp,
) -> Result<(), Failure> {
    let dir = updates_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| failure("Update Failed", e.to_string()))?;
    let dmg = dir.join(format!("AbroMux-{}.dmg", release.version));
    let part = dmg.with_extension("dmg.part");
    let child = Command::new("/usr/bin/curl")
        .args(["-fL", "--silent", "--retry", "2", "--connect-timeout", "30"])
        .arg("-A")
        .arg(user_agent())
        .arg("-o")
        .arg(&part)
        .arg(&release.dmg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| failure("Couldn't Download Update", e.to_string()))?;
    let child = Arc::new(Mutex::new(child));
    let ours = child.clone();
    cx.update(|cx| update(cx, |u| u.download = Some(ours)))
        .map_err(|_| failure("Update Failed", "The app is closing."))?;
    loop {
        Timer::after(Duration::from_millis(150)).await;
        let status = child
            .lock()
            .ok()
            .and_then(|mut c| c.try_wait().ok().flatten());
        let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let live = cx
            .update(|cx| {
                let live = current(generation, cx);
                if live {
                    update(cx, |u| {
                        if let State::Downloading { got: g, .. } = &mut u.state {
                            *g = got;
                        }
                    });
                }
                live
            })
            .unwrap_or(false);
        if !live {
            // Cancelled; `begin` killed curl.
            let _ = std::fs::remove_dir_all(&dir);
            return Ok(());
        }
        if let Some(status) = status {
            if !status.success() {
                return Err(curl_failure(status.code().unwrap_or(-1), true));
            }
            break;
        }
    }
    std::fs::rename(&part, &dmg).map_err(|e| failure("Update Failed", e.to_string()))?;
    log("downloaded; preparing");
    let next = State::Preparing(release.clone());
    cx.update(|cx| update(cx, |u| u.state = next)).ok();
    let staged = {
        let (dmg, app) = (dmg.clone(), app.clone());
        cx.background_executor()
            .spawn(async move { prepare(&dmg, &app) })
            .await?
    };
    if !cx.update(|cx| current(generation, cx)).unwrap_or(false) {
        let _ = std::fs::remove_dir_all(&staged);
        return Ok(());
    }
    log("verified; installing");
    let next = State::Installing(release.clone());
    cx.update(|cx| update(cx, |u| u.state = next)).ok();
    {
        let app = app.clone();
        cx.background_executor()
            .spawn(async move { swap(&staged, &app) })
            .await?;
    }
    let _ = std::fs::remove_dir_all(&dir);
    log(&format!("installed {}; relaunching", release.version));
    relaunch(&app)?;
    cx.update(|cx| cx.quit()).ok();
    Ok(())
}

// ---- the feed -------------------------------------------------------------

fn feed_url() -> String {
    std::env::var("ABROMUX_UPDATE_FEED").unwrap_or_else(|_| {
        let repo = REPO.trim_start_matches("https://github.com/");
        format!("https://api.github.com/repos/{repo}/releases/latest")
    })
}

fn user_agent() -> String {
    format!("AbroMux/{}", env!("ABROMUX_VERSION"))
}

fn current_version() -> String {
    std::env::var("ABROMUX_UPDATE_FROM").unwrap_or_else(|_| env!("ABROMUX_VERSION").into())
}

/// The latest release (blocking).
fn fetch() -> Result<Release, Failure> {
    let out = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "30"])
        .args(["-H", "Accept: application/vnd.github+json"])
        .arg("-A")
        .arg(user_agent())
        .arg(feed_url())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| failure("Update Failed", e.to_string()))?;
    if !out.status.success() {
        return Err(curl_failure(out.status.code().unwrap_or(-1), false));
    }
    parse_release(&String::from_utf8_lossy(&out.stdout))
}

fn parse_release(json: &str) -> Result<Release, Failure> {
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        size: u64,
        browser_download_url: String,
    }
    #[derive(Deserialize)]
    struct Latest {
        tag_name: String,
        html_url: String,
        #[serde(default)]
        published_at: Option<String>,
        #[serde(default)]
        assets: Vec<Asset>,
    }
    let unreadable = || {
        failure(
            "Update Feed Error",
            "The update feed could not be read. Please try again later.",
        )
    };
    let latest: Latest = serde_json::from_str(json).map_err(|_| unreadable())?;
    let version = latest.tag_name.trim_start_matches('v').to_string();
    if parse_version(&version).is_none() {
        return Err(unreadable());
    }
    let dmg = latest
        .assets
        .into_iter()
        .find(|a| a.name.ends_with(".dmg"))
        .ok_or_else(|| {
            failure(
                "Update Failed",
                format!("Release {version} has no disk image to install."),
            )
        })?;
    Ok(Release {
        version,
        url: latest.html_url,
        published: latest.published_at.unwrap_or_default(),
        dmg: dmg.browser_download_url,
        size: dmg.size,
    })
}

/// `0.12.0`, `v0.12.0` or `0.12.0+6dea3c2-dirty` as numbers; build metadata
/// after `+` doesn't count, as in semver.
fn parse_version(v: &str) -> Option<Vec<u64>> {
    let core = v.trim_start_matches('v').split(['+', '-']).next()?;
    core.split('.').map(|n| n.parse().ok()).collect()
}

fn newer(remote: &str, local: &str) -> bool {
    match (parse_version(remote), parse_version(local)) {
        (Some(mut r), Some(mut l)) => {
            let n = r.len().max(l.len());
            r.resize(n, 0);
            l.resize(n, 0);
            r > l
        }
        _ => false,
    }
}

/// What curl's exit code means, in cmux's words.
fn curl_failure(code: i32, download: bool) -> Failure {
    let name = channel::app_name();
    match code {
        6 => failure(
            "Server Not Found",
            "The update server can't be found. Check your connection or try again later.",
        ),
        7 => failure(
            "Server Unreachable",
            format!(
                "{name} couldn't connect to the update server. Check your connection or try again later."
            ),
        ),
        28 => failure(
            "Update Timed Out",
            "The update server took too long to respond. Try again in a moment.",
        ),
        35 | 51 | 53 | 54 | 58 | 59 | 60 | 66 | 77 | 80 | 82 | 83 | 90 | 91 => failure(
            "Secure Connection Failed",
            "A secure connection to the update server couldn't be established. Try again later.",
        ),
        18 | 52 | 55 | 56 => failure(
            "Connection Lost",
            "The network connection was lost while checking for updates. Try again.",
        ),
        _ if download => failure(
            "Couldn't Download Update",
            format!("{name} couldn't download the update. Check your connection and try again."),
        ),
        22 => failure(
            "Update Feed Error",
            format!(
                "{name} couldn't download the update feed. Check your connection and try again."
            ),
        ),
        _ => failure(
            "Update Failed",
            "Something went wrong while checking for updates. Try again, or check the update log for details.",
        ),
    }
}

// ---- installing -----------------------------------------------------------

/// The `.app` this binary runs from.
fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// Why this copy can't replace itself, if it can't.
fn location_problem(app: Option<&Path>) -> Option<Failure> {
    let name = channel::app_name();
    if channel::nightly() {
        return Some(failure(
            "Nightly Build",
            format!(
                "{name} is built from source and doesn't install releases. Update it with scripts/nightly.sh."
            ),
        ));
    }
    let Some(app) = app else {
        return Some(failure(
            "App Location Issue",
            format!("This {name} isn't running from an app bundle, so it can't update itself."),
        ));
    };
    let path = app.to_string_lossy();
    if path.contains("/AppTranslocation/") || path.starts_with("/Volumes/") {
        return Some(failure(
            "App Location Issue",
            format!("Move {name} into Applications and relaunch to enable updates."),
        ));
    }
    let parent = app.parent()?;
    let c = std::ffi::CString::new(parent.as_os_str().as_encoded_bytes()).ok()?;
    if unsafe { libc::access(c.as_ptr(), libc::W_OK) } != 0 {
        return Some(failure(
            "Updater Permission Error",
            format!(
                "{name} can't write to {}. Move {name} into Applications and relaunch to enable updates.",
                parent.display()
            ),
        ));
    }
    None
}

fn run(cmd: &mut Command) -> Result<std::process::Output, String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .trim()
        .to_string())
    }
}

/// An app's code signature: its identifier and Developer ID team, after
/// checking the signature holds (blocking).
fn signature(app: &Path) -> Result<(String, Option<String>), String> {
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(app))?;
    let out = run(Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2"])
        .arg(app))?;
    let info = String::from_utf8_lossy(&out.stderr);
    let field = |key: &str| {
        info.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(str::to_string)
    };
    let team = field("TeamIdentifier=").filter(|t| t != "not set");
    Ok((field("Identifier=").unwrap_or_default(), team))
}

/// Whether an app signed as `new` may replace one signed as `ours`.
fn trusted(
    new: &(String, Option<String>),
    ours: Option<&str>,
    notarized: impl Fn() -> bool,
) -> bool {
    new.0 == BUNDLE_ID
        && match (ours, new.1.as_deref()) {
            (Some(ours), Some(theirs)) => ours == theirs,
            (Some(_), None) => false,
            // This one is ad hoc (built locally): take a notarized release.
            (None, Some(_)) => notarized(),
            (None, None) => false,
        }
}

/// Mount the disk image, check the app in it, and copy it next to `app`
/// (blocking). Returns the copy.
fn prepare(dmg: &Path, app: &Path) -> Result<PathBuf, Failure> {
    let failed = |m: String| failure("Update Failed", m);
    let mount = dmg.with_extension("mount");
    let _ = std::fs::create_dir_all(&mount);
    run(Command::new("/usr/bin/hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-noautoopen",
            "-readonly",
            "-mountpoint",
        ])
        .arg(&mount)
        .arg(dmg))
    .map_err(|e| failed(format!("Couldn't open the disk image: {e}")))?;
    let result = (|| {
        let src = std::fs::read_dir(&mount)
            .map_err(|e| failed(e.to_string()))?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| failed("The disk image has no app in it.".into()))?;
        let bad_signature = || {
            failure(
                "Update Signature Error",
                "The update's signature could not be verified. Please try again later.",
            )
        };
        let theirs = signature(&src).map_err(|e| {
            log(&format!("signature: {e}"));
            bad_signature()
        })?;
        let ours = signature(app).ok().and_then(|s| s.1);
        let notarized = || {
            run(Command::new("/usr/sbin/spctl")
                .args(["--assess", "--type", "execute"])
                .arg(&src))
            .is_ok()
        };
        if !trusted(&theirs, ours.as_deref(), notarized) {
            log(&format!("untrusted: {theirs:?}, ours {ours:?}"));
            return Err(bad_signature());
        }
        let staged = staged_path(app);
        let _ = std::fs::remove_dir_all(&staged);
        run(Command::new("/usr/bin/ditto").arg(&src).arg(&staged))
            .map_err(|e| failed(format!("Couldn't copy the new app: {e}")))?;
        Ok(staged)
    })();
    let _ = run(Command::new("/usr/bin/hdiutil")
        .args(["detach", "-force"])
        .arg(&mount));
    let _ = std::fs::remove_dir(&mount);
    result
}

/// `/Applications/.AbroMux-update.app`: on the same volume, so the swap is a rename.
fn staged_path(app: &Path) -> PathBuf {
    let stem = app.file_stem().unwrap_or_default().to_string_lossy();
    app.with_file_name(format!(".{stem}-update.app"))
}

/// Put the staged app where `app` is (blocking). The running app carries on
/// from the old files, which are gone once it quits.
fn swap(staged: &Path, app: &Path) -> Result<(), Failure> {
    let stem = app.file_stem().unwrap_or_default().to_string_lossy();
    let old = app.with_file_name(format!(".{stem}-old-{}.app", std::process::id()));
    let denied = |e: std::io::Error| {
        failure(
            "Updater Permission Error",
            format!(
                "Couldn't replace {}: {e}. Move {} into Applications and relaunch to enable updates.",
                app.display(),
                channel::app_name()
            ),
        )
    };
    std::fs::rename(app, &old).map_err(denied)?;
    if let Err(e) = std::fs::rename(staged, app) {
        let _ = std::fs::rename(&old, app);
        let _ = std::fs::remove_dir_all(staged);
        return Err(denied(e));
    }
    let _ = std::fs::remove_dir_all(&old);
    // Make Finder, the Dock and Launch Services see the new version.
    let _ = run(Command::new("/usr/bin/touch").arg(app));
    let _ = run(Command::new(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
    )
    .arg("-f")
    .arg(app));
    Ok(())
}

/// Open `app` again once this process has exited.
fn relaunch(app: &Path) -> Result<(), Failure> {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open "$2""#)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(app)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| failure("Update Failed", format!("Couldn't relaunch: {e}")))
}

// ---- saved state ----------------------------------------------------------

fn updates_dir() -> PathBuf {
    channel::support_dir().join("updates")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `update.json` in the support folder.
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Saved {
    /// When the last check ran, in Unix seconds.
    last_check: u64,
    /// The version "Skip" was pressed on.
    skipped: Option<String>,
}

impl Saved {
    fn path() -> PathBuf {
        channel::support_dir().join("update.json")
    }

    fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let _ = std::fs::create_dir_all(channel::support_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }
}

/// `update.log` in the support folder.
fn log(line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(channel::support_dir().join("update.log"))
    {
        let _ = writeln!(f, "{} {line}", now());
    }
}

// ---- the pill and popover -------------------------------------------------

fn visible(state: &State) -> bool {
    !matches!(state, State::Idle | State::Checking { user: false })
}

fn percent(got: u64, total: u64) -> Option<u64> {
    (total > 0).then(|| (got.min(total) * 100) / total)
}

fn pill_text(state: &State) -> String {
    match state {
        State::Idle => String::new(),
        State::Checking { .. } => "Checking for Updates…".into(),
        State::Available(r) => format!("Update Available: {}", r.version),
        State::NotFound => "No Updates Available".into(),
        State::Error(f) => f.title.into(),
        State::Downloading { release, got } => match percent(*got, release.size) {
            Some(p) => format!("Downloading: {p}%"),
            None => "Downloading…".into(),
        },
        State::Preparing(_) => "Preparing…".into(),
        State::Installing(_) => "Installing…".into(),
    }
}

/// The pill, while the updater has something to say. A click opens the popover.
pub fn pill(cx: &App) -> Option<AnyElement> {
    let state = entity(cx).read(cx).state.clone();
    if !visible(&state) {
        return None;
    }
    let available = matches!(state, State::Available(_));
    let (icon_name, color) = match &state {
        State::Available(_) => ("download", 0xffffff),
        State::Error(_) => ("triangle-alert", 0xe5a050),
        State::NotFound => ("info", MUTED),
        State::Installing(_) => ("power", TEXT),
        State::Downloading { .. } => ("download", TEXT),
        _ => ("rotate-cw", TEXT),
    };
    let glyph = icon(icon_name).size(px(11.)).text_color(rgb(color));
    let glyph = if matches!(state, State::Checking { .. } | State::Preparing(_)) {
        glyph
            .with_animation(
                "update-spin",
                Animation::new(Duration::from_secs(1)).repeat(),
                |s, t| s.with_transformation(Transformation::rotate(percentage(t))),
            )
            .into_any_element()
    } else {
        glyph.into_any_element()
    };
    Some(
        div()
            .id("update-pill")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .h(px(22.))
            .px(px(9.))
            .rounded_full()
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .when(available, |d| d.bg(rgb(ACCENT)).text_color(rgb(0xffffff)))
            .when(!available, |d| d.bg(rgba(0xffffff14)).text_color(rgb(TEXT)))
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .active(|s| s.opacity(0.7))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| update(cx, |u| u.open = !u.open))
            .child(glyph)
            .child(div().truncate().child(pill_text(&state)))
            .into_any_element(),
    )
}

/// Where the pill is, for the popover to sit by it.
pub enum Anchor {
    /// The sidebar's foot: the popover opens upward.
    SidebarFoot,
    /// The titlebar row, this far from the window's left edge.
    Titlebar(Pixels),
}

/// The popover by the pill, with a backdrop that closes it.
pub fn popover(anchor: Anchor, top: Pixels, cx: &App) -> Option<AnyElement> {
    let u = entity(cx).read(cx);
    if !u.open || !visible(&u.state) {
        return None;
    }
    let card = div()
        .id("update-popover")
        .absolute()
        .w(px(POPOVER_W))
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .rounded(px(10.))
        .bg(rgb(TABBAR_BG))
        .border_1()
        .border_color(rgb(BORDER))
        .shadow_lg()
        .text_size(px(12.))
        .text_color(rgb(TEXT))
        .occlude()
        .children(content(&u.state));
    let card = match anchor {
        Anchor::SidebarFoot => card.left(px(8.)).bottom(px(42.)),
        Anchor::Titlebar(x) => card.left(x).top(top),
    };
    Some(
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("update-backdrop")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| update(cx, |u| u.open = false)),
            )
            .child(card)
            .into_any_element(),
    )
}

fn title(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

fn muted(text: impl Into<SharedString>) -> Div {
    div().text_color(rgb(MUTED)).child(text.into())
}

fn button(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(10.))
        .py(px(4.))
        .rounded(px(6.))
        .text_size(px(12.))
        .when(primary, |d| d.bg(rgb(ACCENT)).text_color(rgb(0xffffff)))
        .when(!primary, |d| d.bg(rgba(HOVER)))
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .active(|s| s.opacity(0.7))
        .child(label)
}

fn buttons(children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    div()
        .flex()
        .justify_end()
        .gap(px(6.))
        .children(children)
        .into_any_element()
}

fn progress(fraction: Option<f32>) -> AnyElement {
    let bar = div().h_full().rounded_full().bg(rgb(ACCENT));
    let bar = match fraction {
        Some(f) => bar.w(relative(f.clamp(0., 1.))).into_any_element(),
        // No size to go by: a sliver that slides along.
        None => bar
            .w(relative(0.3))
            .with_animation(
                "update-progress",
                Animation::new(Duration::from_millis(1200)).repeat(),
                |d, t| d.ml(relative(0.7 * t)),
            )
            .into_any_element(),
    };
    div()
        .w_full()
        .h(px(5.))
        .rounded_full()
        .bg(rgba(HOVER))
        .flex()
        .child(bar)
        .into_any_element()
}

fn content(state: &State) -> Vec<AnyElement> {
    let name = channel::app_name();
    match state {
        State::Idle => vec![],
        State::Checking { .. } => vec![
            title("Checking for updates…").into_any_element(),
            muted("Please wait while we check for available updates").into_any_element(),
            buttons([button("update-cancel", "Cancel", false)
                .on_click(|_, _, cx| dismiss(cx))
                .into_any_element()]),
        ],
        State::Available(r) => {
            let url = r.url.clone();
            let row = |label: &'static str, value: String| {
                div()
                    .flex()
                    .gap(px(6.))
                    .child(div().w(px(64.)).text_color(rgb(MUTED)).child(label))
                    .child(value)
            };
            vec![
                title("Update Available").into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(row("Version:", r.version.clone()))
                    .when(r.size > 0, |d| d.child(row("Size:", size(r.size))))
                    .when_some(date(&r.published), |d, when| {
                        d.child(row("Released:", when))
                    })
                    .into_any_element(),
                div()
                    .id("update-notes")
                    .text_color(rgb(ACCENT_BRIGHT))
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child("View Release Notes")
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .into_any_element(),
                div()
                    .flex()
                    .gap(px(6.))
                    .child(button("update-skip", "Skip", false).on_click(|_, _, cx| skip(cx)))
                    .child(button("update-later", "Later", false).on_click(|_, _, cx| dismiss(cx)))
                    .child(div().flex_1())
                    .child(
                        button("update-install", "Install and Relaunch", true)
                            .on_click(|_, _, cx| install(cx)),
                    )
                    .into_any_element(),
            ]
        }
        State::Downloading { release, got } => {
            let detail = if release.size > 0 {
                format!("{} of {}", size(*got), size(release.size))
            } else {
                size(*got)
            };
            vec![
                title("Downloading Update").into_any_element(),
                progress(percent(*got, release.size).map(|p| p as f32 / 100.)),
                muted(detail).into_any_element(),
                buttons([button("update-cancel", "Cancel", false)
                    .on_click(|_, _, cx| dismiss(cx))
                    .into_any_element()]),
            ]
        }
        State::Preparing(_) => vec![
            title("Preparing Update").into_any_element(),
            progress(None),
            muted("Extracting and preparing the update").into_any_element(),
        ],
        State::Installing(_) => vec![
            title("Installing…").into_any_element(),
            progress(None),
            muted("Installing update and preparing to restart").into_any_element(),
        ],
        State::NotFound => vec![
            title("No Updates Found").into_any_element(),
            muted("You're already running the latest version.").into_any_element(),
            buttons([button("update-ok", "OK", true)
                .on_click(|_, _, cx| dismiss(cx))
                .into_any_element()]),
        ],
        State::Error(f) => {
            let releases = format!("{REPO}/releases/latest");
            let log_path = channel::support_dir().join("update.log");
            vec![
                title(f.title).text_color(rgb(0xe5a050)).into_any_element(),
                div().child(f.message.clone()).into_any_element(),
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        button("update-log", "Log", false).on_click(move |_, _, cx| {
                            cx.open_url(&format!("file://{}", log_path.display()))
                        }),
                    )
                    .child(
                        button("update-download", "Download Latest Version", false)
                            .on_click(move |_, _, cx| cx.open_url(&releases)),
                    )
                    .child(div().flex_1())
                    .child(button("update-retry", "Retry", true).on_click(|_, _, cx| {
                        check(true, cx);
                    }))
                    .into_any_element(),
                muted(format!("{name} {}", env!("ABROMUX_VERSION")))
                    .text_size(px(10.5))
                    .into_any_element(),
            ]
        }
    }
}

/// `13.4 MB`.
fn size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.;
    if mb >= 1. {
        format!("{mb:.1} MB")
    } else {
        format!("{} KB", bytes / 1000)
    }
}

/// `2026-11-01T12:13:53Z` as `Nov 1, 2026`.
fn date(iso: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = iso.get(..10)?.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    let m: usize = m.parse().ok()?;
    let d: u32 = d.parse().ok()?;
    Some(format!("{} {d}, {y}", MONTHS.get(m.checked_sub(1)?)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui::*` brings its own `test` attribute.
    use core::prelude::v1::test;

    #[test]
    fn versions() {
        assert!(newer("0.13.0", "0.12.0"));
        assert!(newer("v0.12.1", "0.12.0+6dea3c2-dirty"));
        assert!(newer("1.0", "0.99.9"));
        assert!(!newer("0.12.0", "0.12.0+6dea3c2"));
        assert!(!newer("0.12.0", "0.12.0"));
        assert!(!newer("0.11.9", "0.12.0"));
        assert!(!newer("0.12", "0.12.0"));
        assert!(!newer("garbage", "0.12.0"));
    }

    #[test]
    fn release_json() {
        let json = r#"{
            "tag_name": "v0.12.0",
            "html_url": "https://github.com/ZilvinasAbr/abromux/releases/tag/v0.12.0",
            "published_at": "2026-11-01T12:13:53Z",
            "assets": [
                { "name": "notes.txt", "size": 3, "browser_download_url": "https://x/notes.txt" },
                { "name": "AbroMux-0.12.0.dmg", "size": 13388873,
                  "browser_download_url": "https://github.com/ZilvinasAbr/abromux/releases/download/v0.12.0/AbroMux-0.12.0.dmg" }
            ]
        }"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, "0.12.0");
        assert_eq!(r.size, 13388873);
        assert!(r.dmg.ends_with("AbroMux-0.12.0.dmg"));
        assert_eq!(date(&r.published).as_deref(), Some("Nov 1, 2026"));
        assert_eq!(size(r.size), "13.4 MB");
        assert_eq!(
            parse_release(r#"{"tag_name":"v1.0.0","html_url":"","assets":[]}"#)
                .unwrap_err()
                .title,
            "Update Failed"
        );
        assert_eq!(
            parse_release("nope").unwrap_err().title,
            "Update Feed Error"
        );
    }

    #[test]
    fn trust() {
        let sig = |id: &str, team: Option<&str>| (id.to_string(), team.map(str::to_string));
        let yes = || true;
        let no = || false;
        assert!(trusted(&sig(BUNDLE_ID, Some("T1")), Some("T1"), no));
        assert!(!trusted(&sig(BUNDLE_ID, Some("T2")), Some("T1"), yes));
        assert!(!trusted(&sig(BUNDLE_ID, None), Some("T1"), yes));
        assert!(!trusted(&sig("dev.evil", Some("T1")), Some("T1"), yes));
        assert!(trusted(&sig(BUNDLE_ID, Some("T1")), None, yes));
        assert!(!trusted(&sig(BUNDLE_ID, Some("T1")), None, no));
        assert!(!trusted(&sig(BUNDLE_ID, None), None, yes));
    }

    #[test]
    fn pill_texts() {
        let r = Release {
            version: "0.13.0".into(),
            url: String::new(),
            published: String::new(),
            dmg: String::new(),
            size: 200,
        };
        assert_eq!(
            pill_text(&State::Available(r.clone())),
            "Update Available: 0.13.0"
        );
        assert_eq!(
            pill_text(&State::Downloading {
                release: r,
                got: 90
            }),
            "Downloading: 45%"
        );
        assert!(!visible(&State::Checking { user: false }));
        assert!(visible(&State::Checking { user: true }));
    }

    #[test]
    fn staging_sits_beside_the_app() {
        assert_eq!(
            staged_path(Path::new("/Applications/AbroMux.app")),
            Path::new("/Applications/.AbroMux-update.app")
        );
    }

    /// The whole download and verification against the real latest release,
    /// into a scratch folder: `cargo test update::tests::live -- --ignored`.
    #[test]
    #[ignore]
    fn live() {
        let r = fetch().expect("fetch");
        let dir = std::env::temp_dir().join(format!("abromux-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dmg = dir.join("u.dmg");
        let ok = Command::new("/usr/bin/curl")
            .args(["-fsSL", "-o"])
            .arg(&dmg)
            .arg(&r.dmg)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        // Stand-in for the installed app: the release itself, which has the right team.
        let app = dir.join("AbroMux.app");
        let mount = dir.join("m");
        std::fs::create_dir_all(&mount).unwrap();
        run(Command::new("/usr/bin/hdiutil")
            .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
            .arg(&mount)
            .arg(&dmg))
        .unwrap();
        run(Command::new("/usr/bin/ditto")
            .arg(mount.join("AbroMux.app"))
            .arg(&app))
        .unwrap();
        run(Command::new("/usr/bin/hdiutil")
            .args(["detach", "-force"])
            .arg(&mount))
        .unwrap();
        let staged = prepare(&dmg, &app).expect("prepare");
        swap(&staged, &app).expect("swap");
        assert!(app.join("Contents/MacOS/abromux").exists());
        assert!(!staged.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
