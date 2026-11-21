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

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_process_cwd() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(super::cwd(std::process::id()).as_deref(), here.to_str());
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
