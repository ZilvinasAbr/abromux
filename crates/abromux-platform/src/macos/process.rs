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

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_process_cwd() {
        let here = std::env::current_dir().unwrap().canonicalize().unwrap();
        let got = super::cwd(std::process::id()).map(std::path::PathBuf::from);
        assert_eq!(got.map(|p| p.canonicalize().unwrap()), Some(here));
    }
}
