//! GitLab's task lists (`- [ ] item`) in a description, clickable as on
//! GitLab's page: ticking one rewrites that `[ ]` in the description and
//! saves it, showing the tick at once.

/// A task item: its indent, whether it's done, and its text.
#[derive(Debug, PartialEq)]
pub struct Task {
    pub indent: usize,
    pub done: bool,
    pub text: String,
}

/// The `[ ]`/`[x]` of a line that's a task item: its byte range, and done.
fn marker(line: &str) -> Option<(std::ops::Range<usize>, bool)> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let after = if let Some(r) = rest.strip_prefix(['-', '*', '+']) {
        r
    } else {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest[digits..].strip_prefix(['.', ')'])?
    };
    let spaces = after.len() - after.trim_start().len();
    if spaces == 0 {
        return None;
    }
    let at = line.len() - after.len() + spaces;
    let done = match line.get(at..at + 3)? {
        "[ ]" => false,
        "[x]" | "[X]" => true,
        _ => return None,
    };
    // `- [ ]` then a space or the end.
    if !line[at + 3..].is_empty() && !line[at + 3..].starts_with(' ') {
        return None;
    }
    Some((at..at + 3, done))
}

/// A markdown block that's nothing but task items (GitLab draws those as a
/// checklist), split into them.
pub fn parse_block(block: &str) -> Option<Vec<Task>> {
    let mut out = vec![];
    for l in block.lines().filter(|l| !l.trim().is_empty()) {
        let (r, done) = marker(l)?;
        out.push(Task {
            indent: l.len() - l.trim_start().len(),
            done,
            text: l[r.end..].trim().to_string(),
        });
    }
    (!out.is_empty()).then_some(out)
}

/// `body` with its `n`th task item (counting from 0, outside code blocks)
/// ticked or unticked.
pub fn toggle(body: &str, n: usize, done: bool) -> Option<String> {
    let mut seen = 0;
    let mut fence: Option<&str> = None;
    let mut out = String::with_capacity(body.len());
    let mut hit = false;
    for line in body.split_inclusive('\n') {
        let t = line.trim_start();
        if let Some(f) = fence {
            if t.starts_with(f) {
                fence = None;
            }
        } else if t.starts_with("```") || t.starts_with("~~~") {
            fence = Some(&t[..3]);
        } else if let Some((r, _)) = marker(line.trim_end_matches(['\n', '\r'])) {
            if seen == n {
                out.push_str(&line[..r.start]);
                out.push_str(if done { "[x]" } else { "[ ]" });
                out.push_str(&line[r.end..]);
                seen += 1;
                hit = true;
                continue;
            }
            seen += 1;
        }
        out.push_str(line);
    }
    hit.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_ticks_task_lists() {
        let b = parse_block("- [x] limiter\n- [ ] metrics\n  - [ ] nested").unwrap();
        assert_eq!(b.len(), 3);
        assert!(b[0].done && !b[1].done);
        assert_eq!(b[2].indent, 2);
        assert_eq!(b[1].text, "metrics");
        assert!(parse_block("- plain\n- [ ] task").is_none());
        let body = "Intro\n\n```\n- [ ] not a task\n```\n- [x] limiter\n- [ ] metrics\n";
        assert_eq!(
            toggle(body, 1, true).unwrap(),
            "Intro\n\n```\n- [ ] not a task\n```\n- [x] limiter\n- [x] metrics\n"
        );
        assert_eq!(toggle(body, 0, false).unwrap().matches("[ ]").count(), 3);
        assert!(toggle(body, 5, true).is_none());
        assert!(parse_block("1. [ ] first\n2) [X] second").is_some());
    }
}
