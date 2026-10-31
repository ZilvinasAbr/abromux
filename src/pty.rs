//! Where a terminal's program runs, by `"persistence"`:
//!
//! - `Local`: a PTY of abromux's own (`portable-pty`); the shell ends with
//!   the app.
//! - `Hc`: a terminal in herdr-core, relayed over its socket (`crate::hc`).
//! - `Held`: a PTY that abromux's own holder keeps (`crate::hold`), read and
//!   written directly like a local one.
//!
//! Output from any of them goes into a `Feed`, which the view drains on the
//! main thread.

use crate::settings::Persistence;
use futures::channel::mpsc;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

/// A terminal from the last session to take back.
pub enum Attach {
    Hc(String),
    Held(String),
}

/// Output waiting for the main thread, and whether the program is gone.
#[derive(Default)]
pub struct Queue {
    pub bytes: Vec<u8>,
    pub closed: bool,
}

/// Where readers put a program's output: appended to one buffer, with a wake
/// for the view, which takes it all at once (a flood costs one repaint).
#[derive(Clone)]
pub struct Feed {
    queue: Arc<Mutex<Queue>>,
    wake: mpsc::Sender<()>,
}

impl Feed {
    pub fn new() -> (Self, mpsc::Receiver<()>) {
        let (wake, rx) = mpsc::channel(1);
        (
            Self {
                queue: Arc::default(),
                wake,
            },
            rx,
        )
    }

    /// False once the view is gone.
    fn push(&mut self, bytes: &[u8]) -> bool {
        crate::activity::output();
        self.queue.lock().unwrap().bytes.extend_from_slice(bytes);
        !matches!(self.wake.try_send(()), Err(e) if e.is_disconnected())
    }

    fn close(&mut self) {
        self.queue.lock().unwrap().closed = true;
        let _ = self.wake.try_send(());
    }

    pub fn take(&self) -> Queue {
        std::mem::take(&mut *self.queue.lock().unwrap())
    }
}

pub enum Pty {
    Local {
        writer: Box<dyn Write + Send>,
        master: Box<dyn MasterPty + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
    },
    Hc(crate::hc::Terminal),
    Held {
        held: crate::hold::Held,
        file: std::fs::File,
        /// Stops its reader (the write end of a pipe it polls), and the reader.
        stop: Option<(OwnedFd, std::thread::JoinHandle<()>)>,
    },
}

/// A program started or taken back, with what to show first when taken back
/// from the holder.
pub struct Opened {
    pub pty: Pty,
    /// It was running before (a terminal from the last session).
    pub attached: bool,
    /// The holder's terminal as abromux last saw it (`Terminal::snapshot`).
    pub snapshot: Option<Vec<u8>>,
    /// What it printed since.
    pub backlog: Vec<u8>,
}

impl Pty {
    pub fn write(&mut self, bytes: &[u8]) {
        match self {
            Pty::Local { writer, .. } => {
                let _ = writer.write_all(bytes);
                let _ = writer.flush();
            }
            Pty::Hc(t) => t.write(bytes),
            Pty::Held { file, .. } => {
                let _ = file.write_all(bytes);
            }
        }
    }

    pub fn resize(&self, size: PtySize) {
        match self {
            Pty::Local { master, .. } => {
                let _ = master.resize(size);
            }
            Pty::Hc(t) => t.resize(size.cols, size.rows),
            Pty::Held { held, .. } => {
                let ws = libc::winsize {
                    ws_row: size.rows,
                    ws_col: size.cols,
                    ws_xpixel: size.pixel_width,
                    ws_ypixel: size.pixel_height,
                };
                // SAFETY: TIOCSWINSZ reads the winsize it's given.
                unsafe { libc::ioctl(held.fd.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
            }
        }
    }

    /// The shell's pid (herdr-core's and the holder's run on this machine too).
    pub fn pid(&self) -> Option<u32> {
        match self {
            Pty::Local { child, .. } => child.process_id(),
            Pty::Hc(t) => t.pid,
            Pty::Held { held, .. } => Some(held.pid),
        }
    }

    /// End the program for good. Dropping a `Pty` ends a local shell too, but
    /// only lets go of herdr-core's or the holder's.
    pub fn kill(&mut self) {
        match self {
            Pty::Local { child, .. } => {
                let _ = child.kill();
            }
            Pty::Hc(t) => t.kill(),
            Pty::Held { held, stop, .. } => {
                stop_reader(stop);
                held.kill();
            }
        }
    }

    pub fn raw_fd(&self) -> Option<i32> {
        match self {
            Pty::Local { master, .. } => master.as_raw_fd(),
            Pty::Hc(_) => None,
            Pty::Held { held, .. } => Some(held.fd.as_raw_fd()),
        }
    }

    /// herdr-core's address or the holder's id, to take it back next launch.
    pub fn attach(&self) -> Option<Attach> {
        match self {
            Pty::Local { .. } => None,
            Pty::Hc(t) => Some(Attach::Hc(t.address.clone())),
            Pty::Held { held, .. } => Some(Attach::Held(held.id.clone())),
        }
    }

    /// Quitting with a held terminal: stop reading it, so what the view has
    /// processed is all it printed until the holder takes over. Returns its
    /// id; the caller snapshots the screen there.
    pub fn freeze(&mut self) -> Option<String> {
        match self {
            Pty::Held { held, stop, .. } => {
                stop_reader(stop);
                Some(held.id.clone())
            }
            _ => None,
        }
    }
}

fn stop_reader(stop: &mut Option<(OwnedFd, std::thread::JoinHandle<()>)>) {
    if let Some((w, reader)) = stop.take() {
        // SAFETY: one byte to a pipe we own.
        unsafe { libc::write(w.as_raw_fd(), b"x".as_ptr().cast(), 1) };
        let _ = reader.join();
    }
}

/// Start the user's login shell at `size`, or take back `attach`. `vars`
/// is what abromux sets on top of its own environment.
pub fn open(
    cwd: &str,
    shell: &str,
    vars: &[(String, String)],
    size: (u16, u16),
    attach: Option<&Attach>,
    persistence: Persistence,
    feed: &Feed,
) -> Opened {
    let opened = |pty, attached| Opened {
        pty,
        attached,
        snapshot: None,
        backlog: Vec::new(),
    };
    match attach {
        Some(Attach::Hc(address)) => {
            match crate::hc::Terminal::attach(address, size, hc_sink(feed.clone())) {
                Ok(t) => return opened(Pty::Hc(t), true),
                Err(e) => eprintln!("abromux: herdr-core terminal {address}: {e}"),
            }
        }
        Some(Attach::Held(id)) => match crate::hold::Held::attach(id) {
            Ok(mut held) => {
                let path = crate::hold::snapshot_path(id);
                let snapshot = std::fs::read(&path).ok();
                let _ = std::fs::remove_file(path);
                let backlog = std::mem::take(&mut held.backlog);
                if let Some(pty) = held_pty(held, feed) {
                    return Opened {
                        pty,
                        attached: true,
                        snapshot,
                        backlog,
                    };
                }
            }
            Err(e) => eprintln!("abromux: held terminal {id}: {e}"),
        },
        None => {}
    }

    let argv = [shell.to_string(), "-l".into()];
    // herdr-core and the holder start programs with their own environment,
    // so they get all of ours.
    let everything = || -> std::collections::BTreeMap<String, String> {
        std::env::vars().chain(vars.iter().cloned()).collect()
    };
    match persistence {
        Persistence::HerdrCore => {
            match crate::hc::Terminal::spawn(cwd, &argv, &everything(), size, hc_sink(feed.clone())) {
                Ok(t) => return opened(Pty::Hc(t), false),
                Err(e) => eprintln!("abromux: herdr-core: {e}; starting the shell here instead"),
            }
        }
        Persistence::AbromuxHold => {
            let env: Vec<(String, String)> = everything().into_iter().collect();
            match crate::hold::Held::spawn(&argv, cwd, &env, size) {
                Ok(held) => {
                    if let Some(pty) = held_pty(held, feed) {
                        return opened(pty, false);
                    }
                }
                Err(e) => eprintln!("abromux: abromux hold: {e}; starting the shell here instead"),
            }
        }
        Persistence::Abromux => {}
    }
    opened(local(cwd, &argv, vars, size, feed), false)
}

fn local(cwd: &str, argv: &[String], vars: &[(String, String)], (cols, rows): (u16, u16), feed: &Feed) -> Pty {
    let pty = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new(&argv[0]);
    cmd.args(&argv[1..]);
    for (k, v) in vars {
        cmd.env(k, v);
    }
    cmd.cwd(cwd);
    let child = pty.slave.spawn_command(cmd).expect("spawn shell");
    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().expect("pty reader");
    let writer = pty.master.take_writer().expect("pty writer");
    let mut feed = feed.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if !feed.push(&buf[..n]) {
                        return;
                    }
                }
            }
        }
        feed.close();
    });
    Pty::Local {
        writer,
        master: pty.master,
        child,
    }
}

/// A held PTY, read on a thread that `freeze` can stop between reads.
fn held_pty(held: crate::hold::Held, feed: &Feed) -> Option<Pty> {
    let file = std::fs::File::from(held.fd.try_clone().ok()?);
    let read_fd = held.fd.try_clone().ok()?;
    let mut pipe = [0; 2];
    // SAFETY: pipe fills the two descriptors, which we then own.
    if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
        return None;
    }
    use std::os::fd::FromRawFd;
    let (stop_r, stop_w) = unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
    let mut feed = feed.clone();
    let reader = std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut fds = [
                libc::pollfd {
                    fd: read_fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: stop_r.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: two valid pollfds.
            if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if fds[1].revents != 0 {
                return; // frozen: the holder takes over, no exit
            }
            // SAFETY: reads into `buf`, valid for its length.
            let n = unsafe { libc::read(read_fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if n <= 0 {
                break; // the program ended (EIO)
            }
            if !feed.push(&buf[..n as usize]) {
                return;
            }
        }
        feed.close();
    });
    Some(Pty::Held {
        held,
        file,
        stop: Some((stop_w, reader)),
    })
}

/// herdr-core's output into the same feed a local PTY's goes to.
fn hc_sink(mut feed: Feed) -> impl FnMut(crate::hc::Output) -> bool + Send + 'static {
    move |out| match out {
        crate::hc::Output::Bytes(b) => feed.push(&b),
        crate::hc::Output::Closed => {
            feed.close();
            false
        }
    }
}
