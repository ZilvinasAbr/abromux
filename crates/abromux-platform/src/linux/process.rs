//! Other processes.

/// A process's current directory, from `/proc/<pid>/cwd`.
pub fn cwd(pid: u32) -> Option<String> {
    let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    Some(path.to_str()?.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_process_cwd() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(super::cwd(std::process::id()).as_deref(), here.to_str());
    }
}
