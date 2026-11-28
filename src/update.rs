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

use crate::channel;
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

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// The release's page, for its notes.
    pub url: String,
    /// `published_at`, as GitHub gives it.
    pub published: String,
    /// The download that installs it on this OS (the disk image on macOS,
    /// the AppImage on Linux), "" if the release has none; its size, and
    /// its SHA-256 if GitHub gave one.
    pub asset: String,
    pub size: u64,
    pub sha256: Option<String>,
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
    /// Whether the user wants checks on their own
    /// (`app.automaticallyCheckForUpdates`), asked before each one.
    wanted: fn(&App) -> bool,
}

struct Global(Entity<Updater>);
impl gpui::Global for Global {}

/// Start the updater. `wanted` says whether the user wants checks on their
/// own; it's asked before each one, so a changed setting applies at once.
pub fn init(wanted: fn(&App) -> bool, cx: &mut App) {
    let updater = cx.new(|_| Updater {
        state: State::Idle,
        open: false,
        generation: 0,
        download: None,
        wanted,
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
    (entity(cx).read(cx).wanted)(cx)
        && !channel::nightly()
        && if abromux_platform::MACOS {
            bundle().is_some()
        } else {
            appimage().is_some() || release_build()
        }
}

/// A build of a release tag (`0.27.0`), not of a commit (`0.27.0+6dea3c2`):
/// what Linux packages and tarballs are, so they check for updates.
fn release_build() -> bool {
    !env!("ABROMUX_VERSION").contains('+')
}

/// The AppImage this runs from (`$APPIMAGE`, which its runtime sets).
fn appimage() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_file())
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

/// Don't offer this version again, unless the user checks by hand.
pub fn skip(cx: &mut App) {
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
    let app = if abromux_platform::MACOS {
        bundle()
    } else {
        appimage()
    };
    let problem = if abromux_platform::MACOS {
        location_problem(app.as_deref())
    } else {
        linux_problem(app.as_deref(), &release)
    };
    if let Some(f) = problem {
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
    log(&format!("downloading {}", release.asset));
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
    let dmg = dir.join(format!("AbroMux-{}{}", release.version, asset_suffix()));
    let part = dmg.with_extension("part");
    let child = Command::new("/usr/bin/curl")
        .args(["-fL", "--silent", "--retry", "2", "--connect-timeout", "30"])
        .arg("-A")
        .arg(user_agent())
        .arg("-o")
        .arg(&part)
        .arg(&release.asset)
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
    if !abromux_platform::MACOS {
        return install_appimage(&dmg, &release, &app, generation, cx).await;
    }
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

/// The end of the name of the download that installs a release here:
/// `.dmg`, or `-x86_64.AppImage` / `-aarch64.AppImage`.
fn asset_suffix() -> String {
    if abromux_platform::MACOS {
        ".dmg".into()
    } else {
        format!("-{}.AppImage", std::env::consts::ARCH)
    }
}

fn parse_release(json: &str) -> Result<Release, Failure> {
    // On Linux a release without an AppImage still says there's an update.
    parse_release_for(json, &asset_suffix(), abromux_platform::MACOS)
}

/// The release in `json`, with its download ending in `suffix`, which it
/// must have if `required`.
fn parse_release_for(json: &str, suffix: &str, required: bool) -> Result<Release, Failure> {
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        size: u64,
        browser_download_url: String,
        /// `sha256:<hex>`
        #[serde(default)]
        digest: Option<String>,
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
    let asset = latest.assets.into_iter().find(|a| a.name.ends_with(suffix));
    if asset.is_none() && required {
        return Err(failure(
            "Update Failed",
            format!("Release {version} has no disk image to install."),
        ));
    }
    let sha256 = asset
        .as_ref()
        .and_then(|a| a.digest.as_deref()?.strip_prefix("sha256:"))
        .map(str::to_string);
    Ok(Release {
        version,
        url: latest.html_url,
        published: latest.published_at.unwrap_or_default(),
        size: asset.as_ref().map_or(0, |a| a.size),
        asset: asset.map(|a| a.browser_download_url).unwrap_or_default(),
        sha256,
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
    relaunch_with(Path::new("/usr/bin/open"), &[app])
}

/// Run `program` with `args` once this process has exited.
fn relaunch_with(program: &Path, args: &[&Path]) -> Result<(), Failure> {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"pid=$1; shift; while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done; exec "$@""#)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| failure("Update Failed", format!("Couldn't relaunch: {e}")))
}

// ---- Linux ----------------------------------------------------------------

/// Why this Linux copy can't replace itself, if it can't: only an AppImage
/// can. Packages (the AUR's `abromux-bin`) belong to the package manager,
/// and other copies to whoever unpacked them.
fn linux_problem(appimage: Option<&Path>, release: &Release) -> Option<Failure> {
    let name = channel::app_name();
    if channel::nightly() {
        return Some(failure(
            "Nightly Build",
            format!("{name} is built from source and doesn't install releases."),
        ));
    }
    let Some(appimage) = appimage else {
        let exe = std::env::current_exe().unwrap_or_default();
        return Some(if exe.starts_with("/usr") || exe.starts_with("/opt") {
            failure(
                "Installed by Your Package Manager",
                format!(
                    "Update {name} to {} with your package manager, such as `yay -Syu abromux-bin` for the AUR package.",
                    release.version
                ),
            )
        } else {
            failure(
                "Download the Update",
                format!(
                    "This {name} isn't an AppImage, so it can't replace itself. Download {} from the release page.",
                    release.version
                ),
            )
        });
    };
    if release.asset.is_empty() {
        return Some(failure(
            "Update Failed",
            format!(
                "Release {} has no AppImage for this computer.",
                release.version
            ),
        ));
    }
    let dir = appimage.parent()?;
    let c = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).ok()?;
    if unsafe { libc::access(c.as_ptr(), libc::W_OK) } != 0 {
        return Some(failure(
            "Updater Permission Error",
            format!("{name} can't write to {}.", dir.display()),
        ));
    }
    None
}

/// Check the downloaded AppImage, put it in place of the running one (the
/// file can be replaced while it runs), and start it once this quits.
async fn install_appimage(
    download: &Path,
    release: &Release,
    appimage: &Path,
    generation: u64,
    cx: &mut AsyncApp,
) -> Result<(), Failure> {
    let next = State::Preparing(release.clone());
    cx.update(|cx| update(cx, |u| u.state = next)).ok();
    let size = std::fs::metadata(download).map(|m| m.len()).unwrap_or(0);
    if release.size != 0 && size != release.size {
        return Err(failure(
            "Update Failed",
            format!("The download is {size} bytes, not {}.", release.size),
        ));
    }
    if let Some(want) = &release.sha256 {
        let out = run(Command::new("sha256sum").arg(download))
            .map_err(|e| failure("Update Failed", e))?;
        let got = String::from_utf8_lossy(&out.stdout);
        if got.split_whitespace().next() != Some(want.as_str()) {
            return Err(failure(
                "Update Failed",
                "The download doesn't match the release's checksum.",
            ));
        }
    }
    if !cx.update(|cx| current(generation, cx)).unwrap_or(false) {
        return Ok(());
    }
    log("verified; installing");
    let next = State::Installing(release.clone());
    cx.update(|cx| update(cx, |u| u.state = next)).ok();
    // Next to it first, so the rename that replaces it is on one filesystem.
    let staged = appimage.with_extension("AppImage.new");
    let denied = |e: std::io::Error| {
        failure(
            "Updater Permission Error",
            format!("Couldn't replace {}: {e}", appimage.display()),
        )
    };
    std::fs::copy(download, &staged).map_err(denied)?;
    std::fs::set_permissions(&staged, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .map_err(denied)?;
    if let Err(e) = std::fs::rename(&staged, appimage) {
        let _ = std::fs::remove_file(&staged);
        return Err(denied(e));
    }
    let _ = std::fs::remove_dir_all(updates_dir());
    log(&format!("installed {}; relaunching", release.version));
    relaunch_with(appimage, &[])?;
    cx.update(|cx| cx.quit()).ok();
    Ok(())
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
        channel::state_dir().join("update.json")
    }

    fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let _ = std::fs::create_dir_all(channel::state_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }
}

/// `update.log` in the state folder (the support folder on macOS).
fn log(line: &str) {
    let _ = std::fs::create_dir_all(channel::state_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(channel::state_dir().join("update.log"))
    {
        let _ = writeln!(f, "{} {line}", now());
    }
}

/// Whether the updater has something to say: the pill shows.
pub fn visible(state: &State) -> bool {
    !matches!(state, State::Idle | State::Checking { user: false })
}

/// Open or close the pill's popover.
pub fn toggle_popover(cx: &mut App) {
    update(cx, |u| u.open = !u.open);
}

pub fn close_popover(cx: &mut App) {
    update(cx, |u| u.open = false);
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
        let r = parse_release_for(json, ".dmg", true).unwrap();
        assert_eq!(r.version, "0.12.0");
        assert_eq!(r.size, 13388873);
        assert!(r.asset.ends_with("AbroMux-0.12.0.dmg"));
        assert_eq!(
            parse_release_for(
                r#"{"tag_name":"v1.0.0","html_url":"","assets":[]}"#,
                ".dmg",
                true
            )
            .unwrap_err()
            .title,
            "Update Failed"
        );
        // Linux: the AppImage for this architecture, with its checksum; a
        // release without one still says there's an update.
        let linux = r#"{"tag_name":"v0.31.0","html_url":"","assets":[
            {"name":"abromux-0.31.0-x86_64-linux.tar.gz","size":5,"browser_download_url":"https://x/t"},
            {"name":"AbroMux-0.31.0-x86_64.AppImage","size":9,"browser_download_url":"https://x/a",
             "digest":"sha256:abc123"}]}"#;
        let r = parse_release_for(linux, "-x86_64.AppImage", false).unwrap();
        assert_eq!((r.asset.as_str(), r.size), ("https://x/a", 9));
        assert_eq!(r.sha256.as_deref(), Some("abc123"));
        let r = parse_release_for(linux, "-aarch64.AppImage", false).unwrap();
        assert_eq!((r.asset.as_str(), r.size, r.sha256), ("", 0, None));
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
            .arg(&r.asset)
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
