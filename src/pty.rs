//! Where a terminal's program runs, by `"persistence"`:
//!
//! - `Held` (`"abromux"`, the default): a PTY that abromux's holder
//!   keeps (`crate::hold`), so the shell outlives the app; read and written
//!   directly, like a local one.
//! - `Local` (`"cmux-like"`): a PTY of abromux's own (`portable-pty`); the
//!   shell ends with the app.
//!
//! Output from either goes into a `Feed`, which the view drains on the main
//! thread.

use crate::settings::Persistence;
use futures::channel::mpsc;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};

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
    Held {
        held: crate::hold::Held,
        file: std::fs::File,
        /// Stops its reader (`freeze`).
        stop: Option<Stop>,
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

    /// The shell's pid (the holder's run on this machine too).
    pub fn pid(&self) -> Option<u32> {
        match self {
            Pty::Local { child, .. } => child.process_id(),
            Pty::Held { held, .. } => Some(held.pid),
        }
    }

    /// End the program for good. Dropping a `Pty` ends a local shell too, but
    /// only lets go of a held one.
    pub fn kill(&mut self) {
        match self {
            Pty::Local { child, .. } => {
                let _ = child.kill();
            }
            Pty::Held { held, stop, .. } => {
                stop_reader(stop);
                held.kill();
            }
        }
    }

    pub fn raw_fd(&self) -> Option<i32> {
        match self {
            Pty::Local { master, .. } => master.as_raw_fd(),
            Pty::Held { held, .. } => Some(held.fd.as_raw_fd()),
        }
    }

    /// The holder's id for it, to take it back next launch.
    pub fn held_id(&self) -> Option<&str> {
        match self {
            Pty::Local { .. } => None,
            Pty::Held { held, .. } => Some(&held.id),
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

fn stop_reader(stop: &mut Option<Stop>) {
    use std::os::unix::thread::JoinHandleExt;
    if let Some((stopped, reader)) = stop.take() {
        stopped.store(true, std::sync::atomic::Ordering::Release);
        // Interrupt its blocking read (EINTR). Again until it's out: the
        // signal can land just before it enters read.
        while !reader.is_finished() {
            // SAFETY: the thread is alive (not finished) and the handler is set.
            unsafe { libc::pthread_kill(reader.as_pthread_t(), WAKE) };
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let _ = reader.join();
    }
}

/// How `freeze` stops a held PTY's reader: a flag it checks after each read,
/// and the thread, whose blocking read `WAKE` interrupts.
type Stop = (Arc<std::sync::atomic::AtomicBool>, std::thread::JoinHandle<()>);

/// The signal that interrupts a held reader's read. Its handler does
/// nothing and is installed without SA_RESTART, so read returns EINTR.
const WAKE: libc::c_int = libc::SIGUSR2;

fn install_wake() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    extern "C" fn nothing(_: libc::c_int) {}
    ONCE.call_once(|| {
        // SAFETY: a handler that does nothing, without SA_RESTART.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = nothing as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = 0;
            libc::sigaction(WAKE, &sa, std::ptr::null_mut());
        }
    });
}

/// Start the user's login shell at `size`, or take back the holder's
/// terminal `attach`. `vars` is what abromux sets on top of its own
/// environment.
pub fn open(
    cwd: &str,
    shell: &str,
    vars: &[(String, String)],
    size: (u16, u16),
    attach: Option<&str>,
    persistence: Persistence,
    feed: &Feed,
) -> Opened {
    let opened = |pty| Opened {
        pty,
        attached: false,
        snapshot: None,
        backlog: Vec::new(),
    };
    if let Some(id) = attach {
        match crate::hold::Held::attach(id) {
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
        }
    }

    let argv = [shell.to_string(), "-l".into()];
    if persistence == Persistence::Abromux {
        // The holder starts programs with exactly the environment it's
        // given, so it gets all of ours.
        let env: Vec<(String, String)> = std::env::vars()
            .chain(vars.iter().cloned())
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_iter()
            .collect();
        match crate::hold::Held::spawn(&argv, cwd, &env, size) {
            Ok(held) => {
                if let Some(pty) = held_pty(held, feed) {
                    return opened(pty);
                }
            }
            Err(e) => eprintln!("abromux: abromux hold: {e}; starting the shell here instead"),
        }
    }
    opened(local(cwd, &argv, vars, size, feed))
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
///
/// It reads exactly as a local PTY's reader does, one blocking read per
/// chunk: macOS PTYs give small chunks, so anything more per chunk (a poll
/// first, or a non-blocking read that comes back empty) costs throughput.
/// `freeze` interrupts the read with a signal instead.
fn held_pty(held: crate::hold::Held, feed: &Feed) -> Option<Pty> {
    install_wake();
    let read_fd = held.fd.try_clone().ok()?;
    // SAFETY: fcntl on a descriptor we own: blocking, whatever an earlier
    // owner left it as.
    unsafe {
        let flags = libc::fcntl(read_fd.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(read_fd.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK);
    }
    let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag = stopped.clone();
    let mut feed = feed.clone();
    let reader = std::thread::spawn(move || {
        use std::sync::atomic::Ordering;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            // SAFETY: reads into `buf`, valid for its length.
            let n = unsafe { libc::read(read_fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 && !feed.push(&buf[..n as usize]) {
                return;
            }
            if stop_flag.load(Ordering::Acquire) {
                return; // frozen: the holder takes over, no exit
            }
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if n <= 0 {
                break; // the program ended (EIO)
            }
        }
        feed.close();
    });
    Some(Pty::Held {
        file: std::fs::File::from(held.fd.try_clone().ok()?),
        stop: Some((stopped, reader)),
        held,
    })
}
