//! Other processes.

/// A process's current directory, from `/proc/<pid>/cwd`.
pub fn cwd(pid: u32) -> Option<String> {
    let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    Some(path.to_str()?.to_string())
}

/// A process's direct children: the processes whose parent (the field
/// after the command name in `/proc/<pid>/stat`) is `pid`.
pub fn children(pid: u32) -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|p| {
            std::fs::read_to_string(format!("/proc/{p}/stat"))
                .ok()
                .and_then(|stat| {
                    let rest = &stat[stat.rfind(')')? + 1..];
                    rest.split_whitespace().nth(1)?.parse::<u32>().ok()
                })
                == Some(pid)
        })
        .collect()
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

/// A process's parent, user and start time, from `/proc/<pid>/stat`
/// (fields 4 and 22) and the owner of `/proc/<pid>`.
pub fn info(pid: u32) -> Option<Info> {
    use std::os::unix::fs::MetadataExt as _;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the command name, which may hold spaces and parens.
    let rest: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    Some(Info {
        parent: rest.get(1)?.parse().ok()?,
        uid: std::fs::metadata(format!("/proc/{pid}")).ok()?.uid(),
        started: rest.get(19)?.parse().ok()?,
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
        let here = std::env::current_dir().unwrap();
        assert_eq!(super::cwd(std::process::id()).as_deref(), here.to_str());
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
