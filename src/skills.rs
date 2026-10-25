//! Agent skills that teach Claude Code and Codex the `abromux` CLI. The
//! SKILL.md files live in `skills/` and are compiled in, so the installed
//! skills always describe the binary that installed them.
//!
//! `abromux skills install` writes them to `~/.claude/skills` (or
//! `$CLAUDE_CONFIG_DIR/skills`) and, if Codex is set up, `~/.codex/skills`
//! (or `$CODEX_HOME/skills`). `abromux skills show [name]` prints one.

use anyhow::{Context as _, bail};
use std::path::{Path, PathBuf};

const SKILLS: &[(&str, &str)] = &[
    ("abromux", include_str!("../skills/abromux/SKILL.md")),
    (
        "abromux-browser",
        include_str!("../skills/abromux-browser/SKILL.md"),
    ),
];

/// Ends every installed SKILL.md, so uninstall and reinstall only touch ours.
const MARK: &str = "<!-- installed by `abromux skills install`";

fn installed(text: &str) -> String {
    format!(
        "{text}\n{MARK} {}; rerun it after updating abromux -->\n",
        env!("ABROMUX_VERSION")
    )
}

/// Each agent's skills folder that exists or can be made, with its name.
fn roots() -> anyhow::Result<Vec<(&'static str, PathBuf)>> {
    let home = || {
        std::env::var("HOME")
            .map(PathBuf::from)
            .context("HOME isn't set")
    };
    let claude = match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => home()?.join(".claude"),
    };
    let codex = match std::env::var("CODEX_HOME") {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => home()?.join(".codex"),
    };
    let mut out = vec![("Claude Code", claude.join("skills"))];
    if codex.is_dir() {
        out.push(("Codex", codex.join("skills")));
    }
    Ok(out)
}

/// `abromux skills install|uninstall|show [name]`.
pub fn run(pos: &[String]) -> anyhow::Result<String> {
    match pos.first().map(String::as_str) {
        Some("install") => edit(true),
        Some("uninstall") => edit(false),
        Some("show") => {
            let name = pos.get(1).map_or("abromux", String::as_str);
            match SKILLS.iter().find(|(n, _)| *n == name) {
                Some((_, text)) => Ok(text.trim_end().into()),
                None => bail!("no skill {name:?} (abromux, abromux-browser)"),
            }
        }
        _ => bail!("skills: expected install, uninstall or show [name]"),
    }
}

fn edit(install: bool) -> anyhow::Result<String> {
    let mut out = vec![];
    for (agent, root) in roots()? {
        let mut done = vec![];
        for (name, text) in SKILLS {
            let dir = root.join(name);
            if !ours(&dir)? {
                out.push(format!(
                    "Skipped {}: it isn't one abromux installed",
                    dir.display()
                ));
                continue;
            }
            if install {
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join("SKILL.md"), installed(text))
                    .with_context(|| format!("can't write {}", dir.display()))?;
            } else if dir.exists() {
                std::fs::remove_dir_all(&dir)
                    .with_context(|| format!("can't remove {}", dir.display()))?;
            } else {
                continue;
            }
            done.push(*name);
        }
        if !done.is_empty() {
            out.push(format!(
                "{} {agent} skills {} in {}",
                if install { "Installed" } else { "Removed" },
                done.join(", "),
                root.display()
            ));
        } else if !install {
            out.push(format!("No {agent} skills to remove in {}", root.display()));
        }
    }
    Ok(out.join("\n"))
}

/// Whether `dir` is missing or holds only a SKILL.md we wrote.
fn ours(dir: &Path) -> anyhow::Result<bool> {
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e).with_context(|| format!("can't read {}", dir.display())),
        Ok(m) if !m.is_dir() => return Ok(false),
        Ok(_) => {}
    }
    let only_skill = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .all(|e| e.file_name() == "SKILL.md");
    let marked = std::fs::read_to_string(dir.join("SKILL.md")).is_ok_and(|t| t.contains(MARK));
    Ok(only_skill && marked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_have_frontmatter_named_after_their_folder() {
        for (name, text) in SKILLS {
            assert!(text.starts_with(&format!("---\nname: {name}\n")), "{name}");
            assert!(text.contains("\ndescription: "), "{name}");
        }
    }

    #[test]
    fn only_touches_skills_it_installed() {
        let tmp = std::env::temp_dir().join(format!("abromux-skills-{}", std::process::id()));
        let dir = tmp.join("abromux");
        assert!(ours(&dir).unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "someone else's").unwrap();
        assert!(!ours(&dir).unwrap());
        std::fs::write(dir.join("SKILL.md"), installed(SKILLS[0].1)).unwrap();
        assert!(ours(&dir).unwrap());
        std::fs::write(dir.join("notes.md"), "").unwrap();
        assert!(!ours(&dir).unwrap());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
