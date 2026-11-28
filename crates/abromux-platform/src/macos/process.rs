//! Other processes.

/// A process's current directory, via libproc's PROC_PIDVNODEPATHINFO.
pub fn cwd(pid: u32) -> Option<String> {
    unsafe extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, size: i32) -> i32;
    }
    const PROC_PIDVNODEPATHINFO: i32 = 9;
    // struct proc_vnodepathinfo { vnode_info_path cdir, rdir }, where
    // vnode_info_path is a 152-byte vnode_info followed by char[MAXPATHLEN].
    const VNODE_INFO: usize = 152;
    const MAXPATHLEN: usize = 1024;
    let mut buf = [0u8; 2 * (VNODE_INFO + MAXPATHLEN)];
    let n = unsafe {
        proc_pidinfo(
            pid as i32,
            PROC_PIDVNODEPATHINFO,
            0,
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    if n as usize != buf.len() {
        return None;
    }
    let path = &buf[VNODE_INFO..VNODE_INFO + MAXPATHLEN];
    let end = path.iter().position(|&b| b == 0)?;
    (end > 0).then(|| String::from_utf8_lossy(&path[..end]).into_owned())
}

/// A process's direct children, via libproc's proc_listchildpids.
pub fn children(pid: u32) -> Vec<u32> {
    unsafe extern "C" {
        fn proc_listchildpids(ppid: i32, buffer: *mut i32, buffersize: i32) -> i32;
    }
    let mut buf = [0i32; 256];
    let size = std::mem::size_of_val(&buf) as i32;
    let n = unsafe { proc_listchildpids(pid as i32, buf.as_mut_ptr(), size) };
    if n <= 0 {
        return vec![];
    }
    // Zeroed first, so the pids it wrote are the nonzero ones whichever
    // unit (pids or bytes) it counts in.
    buf.iter().filter(|&&p| p > 0).map(|&p| p as u32).collect()
}

/// What the terminal checks about a process that reports to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Info {
    pub parent: u32,
    /// Its effective user.
    pub uid: u32,
    /// When it started, in a unit of the OS's own: only for telling a
    /// process from a later one with the same pid.
    pub started: u64,
}

/// A process's parent, user and start time, via libproc's PROC_PIDTBSDINFO.
pub fn info(pid: u32) -> Option<Info> {
    unsafe extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, size: i32) -> i32;
    }
    const PROC_PIDTBSDINFO: i32 = 3;
    // struct proc_bsdinfo: pbi_ppid at 16, pbi_uid at 20, then after the
    // names and counters pbi_start_tvsec at 120 and pbi_start_tvusec at 128.
    let mut buf = [0u8; 136];
    let n = unsafe {
        proc_pidinfo(
            pid as i32,
            PROC_PIDTBSDINFO,
            0,
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    if n as usize != buf.len() {
        return None;
    }
    let u32_at = |i: usize| u32::from_ne_bytes(buf[i..i + 4].try_into().unwrap());
    let u64_at = |i: usize| u64::from_ne_bytes(buf[i..i + 8].try_into().unwrap());
    Some(Info {
        parent: u32_at(16),
        uid: u32_at(20),
        started: u64_at(120)
            .wrapping_mul(1_000_000)
            .wrapping_add(u64_at(128)),
    })
}

/// This process's effective user.
pub fn uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_process_cwd() {
        let here = std::env::current_dir().unwrap().canonicalize().unwrap();
        let got = super::cwd(std::process::id()).map(std::path::PathBuf::from);
        assert_eq!(got.map(|p| p.canonicalize().unwrap()), Some(here));
    }

    #[test]
    fn reads_a_process_info() {
        let me = super::info(std::process::id()).unwrap();
        assert_eq!(me.uid, super::uid());
        assert_eq!(me.parent, std::os::unix::process::parent_id());
        assert_eq!(super::info(std::process::id()), Some(me));
        assert!(super::info(u32::MAX).is_none());
    }

    #[test]
    fn lists_a_process_children() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let kids = super::children(std::process::id());
        let _ = child.kill();
        assert!(kids.contains(&child.id()));
    }
}
