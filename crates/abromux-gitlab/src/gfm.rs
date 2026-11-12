//! GitLab's markdown made into what the markdown renderer draws well:
//! what GitLab hides is taken out (template comments, `[[_TOC_]]`), what it
//! resolves against the project is resolved (uploads, `/group/project`
//! links), and what the renderer has no element for is written with ones
//! it has (`<details>`, `>>>` quotes, pictures only a signed-in browser
//! can fetch).

use regex::{Captures, Regex};
use std::sync::LazyLock;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

static COMMENT: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)<!--.*?-->"));
static TOC: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^[ \t]*(\[\[_TOC_\]\]|\[TOC\])[ \t]*$"));
static SUMMARY: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<summary[^>]*>(.*?)</summary>"));
static DETAILS: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)</?details[^>]*>"));
static TAG: LazyLock<Regex> = LazyLock::new(|| re(r"</?[a-zA-Z][^>]*>"));
/// `![alt](target "title"){width=50%}`: 1 alt, 2 target.
static IMAGE: LazyLock<Regex> = LazyLock::new(|| re(r#"!\[([^\]]*)\]\(\s*<?([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)(?:\{[^}\n]*\})?"#));
static IMG_TAG: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)<img\b[^>]*?\bsrc\s*=\s*["']([^"']+)["'][^>]*>"#));
static ALT_ATTR: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)\balt\s*=\s*["']([^"']*)["']"#));
/// `](/path)`: a link to somewhere on this GitLab.
static RELATIVE: LazyLock<Regex> = LazyLock::new(|| re(r"\]\(\s*(/[^)\s]*)"));
/// `**[text](url)**` (also `__`, `*`, `_`): 1 the marker, 2 text, 3 url.
static STRONG_LINK: LazyLock<Regex> = LazyLock::new(|| re(r"(\*\*|__|\*|_)\[([^\]\n]+)\]\(([^)\n]+)\)(\*\*|__|\*|_)"));
/// A line that ends in a hard break: two spaces or a backslash.
static HARD_BREAK: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)(\S)(?: {2,}|\\)$"));
static BLANKS: LazyLock<Regex> = LazyLock::new(|| re(r"\n{3,}"));

/// Where `target` (a link's or a picture's) is: an upload under the
/// project, a path on the GitLab `host`, or as written.
fn resolve(target: &str, project: &str, host: &str) -> Option<String> {
    let path = target.strip_prefix('/').unwrap_or(target);
    if path.starts_with("uploads/") && !project.is_empty() {
        return Some(format!("{project}/{path}"));
    }
    (target.starts_with('/') && !target.starts_with("//") && !host.is_empty()).then(|| format!("{host}{target}"))
}

/// A picture's name for its link: its alt text, else its file's name.
fn picture_label(alt: &str, target: &str) -> String {
    let alt = alt.trim();
    let name = if alt.is_empty() || alt.eq_ignore_ascii_case("image") {
        target.split(['?', '#']).next().unwrap_or(target).rsplit('/').next().unwrap_or(target)
    } else {
        alt
    };
    // Brackets would end the link's text.
    format!("🖼 {}", name.replace(['[', ']'], ""))
}

/// `>>>` … `>>>`: GitLab's quote of several lines, as `>` lines.
fn block_quotes(md: &str) -> String {
    if !md.lines().any(|l| l.trim() == ">>>") {
        return md.to_string();
    }
    let mut out = String::with_capacity(md.len());
    let mut quoting = false;
    for l in md.lines() {
        if l.trim() == ">>>" {
            quoting = !quoting;
            out.push('\n');
            continue;
        }
        if quoting {
            out.push_str("> ");
        }
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// `md` (GitLab's, without its fenced code blocks) ready for the renderer.
/// `project` is the project's page (`https://gl/group/project`), `host` the
/// GitLab; either may be empty (a local diff), and then nothing is resolved.
pub fn prepare(md: &str, project: &str, host: &str) -> String {
    let md = COMMENT.replace_all(md, "");
    let md = TOC.replace_all(&md, "");
    let md = block_quotes(&md);
    // A folded section: its summary as a heading line, its text below
    // (always open: there's nothing to fold it with).
    let md = SUMMARY.replace_all(&md, |c: &Captures| {
        let text = TAG.replace_all(c[1].trim(), "");
        format!("\n\n**▸ {}**\n\n", text.trim())
    });
    let md = DETAILS.replace_all(&md, "\n\n");
    // Pictures uploaded to GitLab need its session to fetch: a link to each
    // instead, which the browser opens. Others stay pictures.
    let md = IMAGE.replace_all(&md, |c: &Captures| match resolve(&c[2], project, host) {
        Some(url) => format!("[{}]({url})", picture_label(&c[1], &c[2])),
        None => format!("![{}]({})", &c[1], &c[2]),
    });
    let md = IMG_TAG.replace_all(&md, |c: &Captures| match resolve(&c[1], project, host) {
        Some(url) => {
            let alt = ALT_ATTR.captures(&c[0]).map(|a| a[1].to_string()).unwrap_or_default();
            format!("[{}]({url})", picture_label(&alt, &c[1]))
        }
        None => c[0].to_string(),
    });
    let md = RELATIVE.replace_all(&md, |c: &Captures| match resolve(&c[1], project, host) {
        Some(url) => format!("]({url}"),
        None => c[0].to_string(),
    });
    // The renderer drops a link inside bold or italics: the other way round.
    let md = STRONG_LINK.replace_all(&md, |c: &Captures| {
        if c[1] == c[4] { format!("[{m}{}{m}]({})", &c[2], &c[3], m = &c[1]) } else { c[0].to_string() }
    });
    // …and a hard break altogether, where a line's end breaks the line
    // anyway (as in GitLab's comments).
    let md = HARD_BREAK.replace_all(&md, "$1");
    BLANKS.replace_all(md.trim(), "\n\n").into_owned()
}

/// `md` as its top-level blocks (paragraphs, headings, whole lists, tables,
/// quotes), each drawn on its own with GitLab's space between: the renderer
/// leaves none between the blocks of one text. Whole if it has link
/// definitions (`[name]: url`), which the blocks using them need beside them.
pub fn blocks(md: &str) -> Vec<String> {
    use markdown::mdast::Node;
    let whole = || vec![md.to_string()];
    let Ok(Node::Root(root)) = markdown::to_mdast(md, &markdown::ParseOptions::gfm()) else { return whole() };
    if root.children.iter().any(|n| matches!(n, Node::Definition(_) | Node::FootnoteDefinition(_))) {
        return whole();
    }
    let mut out = vec![];
    for n in &root.children {
        let Some(pos) = n.position() else { return whole() };
        let Some(text) = md.get(pos.start.offset..pos.end.offset) else { return whole() };
        if !text.trim().is_empty() {
            out.push(text.trim_end().to_string());
        }
    }
    if out.is_empty() { whole() } else { out }
}

#[cfg(test)]
mod tests {
    use super::prepare;

    #[test]
    fn splits_into_blocks() {
        let b = super::blocks("# T\n\nOne\ntwo\n\n- a\n\n- b\n  - c\n\n| x |\n|---|\n| y |\n\n> q\n> r");
        assert_eq!(b, ["# T", "One\ntwo", "- a\n\n- b\n  - c", "| x |\n|---|\n| y |", "> q\n> r"]);
        // With a definition its links need, it stays whole.
        assert_eq!(super::blocks("See [a].\n\n[a]: http://x").len(), 1);
        assert_eq!(super::blocks("ž one\n\nū two"), ["ž one", "ū two"]);
    }

    fn p(md: &str) -> String {
        prepare(md, "https://gl.test/g/p", "https://gl.test")
    }

    #[test]
    fn hides_what_gitlab_hides() {
        assert_eq!(p("<!-- Describe the change\nhere -->\n## What\n\nIt works.<!-- x -->"), "## What\n\nIt works.");
        assert_eq!(p("[[_TOC_]]\n\n# A\n\n[TOC]\n\ntext [TOC] stays"), "# A\n\ntext [TOC] stays");
    }

    #[test]
    fn folded_sections_show_their_summary() {
        assert_eq!(
            p("<details>\n<summary>Click <b>me</b></summary>\n\n- a\n- b\n\n</details>\nafter"),
            "**▸ Click me**\n\n- a\n- b\n\nafter"
        );
    }

    #[test]
    fn uploads_and_relative_links_resolve() {
        assert_eq!(
            p("![shot](/uploads/abc123/shot.png){width=400}"),
            "[🖼 shot](https://gl.test/g/p/uploads/abc123/shot.png)"
        );
        assert_eq!(p("![image](uploads/abc/a b.png)"), "![image](uploads/abc/a b.png)", "a target with a space isn't one");
        assert_eq!(p("![image](uploads/abc/ab.png)"), "[🖼 ab.png](https://gl.test/g/p/uploads/abc/ab.png)");
        assert_eq!(p("<img src=\"/uploads/abc/x.png\" alt=\"Before\" width=\"300\">"), "[🖼 Before](https://gl.test/g/p/uploads/abc/x.png)");
        // Elsewhere: still a picture.
        assert_eq!(p("![logo](https://x.test/logo.png)"), "![logo](https://x.test/logo.png)");
        assert_eq!(p("[the docs](/g/p/-/wikis/home) and [x](https://x.test/a)"), "[the docs](https://gl.test/g/p/-/wikis/home) and [x](https://x.test/a)");
        // A local diff has no GitLab to resolve against.
        assert_eq!(prepare("[a](/b) ![c](/uploads/d.png)", "", ""), "[a](/b) ![c](/uploads/d.png)");
    }

    #[test]
    fn quotes_bold_links_and_breaks() {
        assert_eq!(p("He said:\n>>>\nline one\n\nline two\n>>>\nok"), "He said:\n\n> line one\n> \n> line two\n\nok");
        assert_eq!(p("**[Jira](https://j.test/X-1)** and _[b](https://b.test)_"), "[**Jira**](https://j.test/X-1) and [_b_](https://b.test)");
        assert_eq!(p("one  \ntwo\\\nthree\n\n\n\nfour"), "one\ntwo\nthree\n\nfour");
        // Left as they are.
        assert_eq!(p("a * b [c](d) * e"), "a * b [c](d) * e");
    }
}
