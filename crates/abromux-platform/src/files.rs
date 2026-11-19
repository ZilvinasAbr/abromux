//! How many files the app may have open at once.
//!
//! Every terminal tab holds a pty and a few copies of it, so the soft limit
//! a GUI app starts with runs out after a few dozen tabs: 256 on macOS when
//! launched from Finder or the Dock (launchd's default), often 1024 on Linux.
//! Opening another tab then fails with "Too many open files" (EMFILE).
//! [`raise_open_files_limit`] lifts the soft limit to what the hard limit
//! allows, as other terminal apps do.

/// Asked for when the hard limit is unlimited; also macOS's `OPEN_MAX`, the
/// most `setrlimit` takes there for the soft limit on some versions.
const FALLBACK: libc::rlim_t = 10240;

/// Raises this process's soft limit on open files as far as the hard limit
/// allows, and returns the new soft limit, or `None` if it couldn't change
/// it. Never lowers it. Shells started afterwards inherit the new limit.
pub fn raise_open_files_limit() -> Option<libc::rlim_t> {
    let mut rl = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit only writes the struct we pass.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rl) } != 0 {
        return None;
    }
    let want = if rl.rlim_max == libc::RLIM_INFINITY {
        FALLBACK.max(per_process_max().unwrap_or(0))
    } else {
        rl.rlim_max
    };
    if rl.rlim_cur != libc::RLIM_INFINITY && rl.rlim_cur >= want {
        return Some(rl.rlim_cur);
    }
    // macOS refuses a soft limit over kern.maxfilesperproc (and, on some
    // versions, over OPEN_MAX), so step down until one is taken.
    for cur in [
        want,
        per_process_max().unwrap_or(want).min(want),
        FALLBACK.min(want),
    ] {
        if cur <= rl.rlim_cur {
            continue;
        }
        let new = libc::rlimit {
            rlim_cur: cur,
            rlim_max: rl.rlim_max,
        };
        // SAFETY: setrlimit only reads the struct we pass.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &new) } == 0 {
            return Some(cur);
        }
    }
    None
}

/// The kernel's per-process cap on open files, where there is one to ask.
#[cfg(target_os = "macos")]
fn per_process_max() -> Option<libc::rlim_t> {
    let mut value: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is NUL-terminated and value/size describe a c_int.
    let ok = unsafe {
        libc::sysctlbyname(
            c"kern.maxfilesperproc".as_ptr(),
            (&mut value as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    (ok && value > 0).then_some(value as libc::rlim_t)
}

#[cfg(not(target_os = "macos"))]
fn per_process_max() -> Option<libc::rlim_t> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn soft() -> libc::rlim_t {
        let mut rl = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rl) }, 0);
        rl.rlim_cur
    }

    #[test]
    fn raises_a_low_soft_limit() {
        // Start where a Finder-launched app does.
        let mut rl = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rl) };
        let low = libc::rlimit {
            rlim_cur: 256.min(rl.rlim_max),
            rlim_max: rl.rlim_max,
        };
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &low) }, 0);

        let raised = raise_open_files_limit().expect("raised");
        assert_eq!(soft(), raised);
        if rl.rlim_max > 256 {
            assert!(raised > 256, "still {raised}");
        }
        // Again is a no-op, never lower.
        assert_eq!(raise_open_files_limit(), Some(raised));
    }
}
