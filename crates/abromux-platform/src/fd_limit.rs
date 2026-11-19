//! How many files the process may have open. Every terminal holds a few
//! (its PTY, a reader's copy, the holder's socket), and the browser panes
//! more, while macOS starts an app from Finder or the Dock with a soft limit
//! of 256: past about fifty terminals a new one failed with "Too many open
//! files". [`raise`] lifts the soft limit to what the system allows, as other
//! terminals do; what abromux starts inherits it.

/// Raise the soft limit on open files to the hard one (on macOS, at most
/// `kern.maxfilesperproc`). Best effort: a failure keeps the old limit.
pub fn raise() {
    // SAFETY: getrlimit/setrlimit read and write the struct they're given.
    unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return;
        }
        let want = cap(lim.rlim_max);
        if lim.rlim_cur >= want {
            return;
        }
        let cur = lim.rlim_cur;
        lim.rlim_cur = want;
        if libc::setrlimit(libc::RLIMIT_NOFILE, &lim) != 0 {
            eprintln!(
                "abromux: couldn't raise the open-file limit from {cur} to {want}: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

/// macOS refuses a soft limit above the per-process maximum, though the hard
/// limit is usually unlimited.
#[cfg(target_os = "macos")]
fn cap(hard: libc::rlim_t) -> libc::rlim_t {
    let mut max: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    // SAFETY: sysctlbyname writes at most `len` bytes into `max`.
    let ok = unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            (&raw mut max).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    // OPEN_MAX: what macOS took before it had the sysctl.
    let max = if ok && max > 0 {
        max as libc::rlim_t
    } else {
        10240
    };
    hard.min(max)
}

#[cfg(not(target_os = "macos"))]
fn cap(hard: libc::rlim_t) -> libc::rlim_t {
    hard
}

#[cfg(test)]
mod tests {
    #[test]
    fn raises_the_soft_limit_to_the_cap() {
        super::raise();
        // SAFETY: as in `raise`.
        let lim = unsafe {
            let mut lim: libc::rlimit = std::mem::zeroed();
            assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim), 0);
            lim
        };
        assert_eq!(lim.rlim_cur, super::cap(lim.rlim_max));
    }
}
