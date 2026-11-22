//! The control socket's methods described (abromux_cli::api), checked
//! here against the handlers in server.rs and the server's lists.

pub use abromux_cli::api::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The methods `server.rs` handles, from its `match method` arms.
    fn handled() -> Vec<String> {
        let src = include_str!("server.rs");
        let start = src.find("let v = match method {").expect("the match");
        let body = &src[start..];
        let end = body
            .find("_ => return Err(MethodNotFound")
            .expect("its end");
        let mut out = vec![];
        for line in body[..end].lines() {
            let t = line.trim_start();
            // An arm's pattern: `"a.b" =>`, `"a.b" | "c.d" =>`, or one per line.
            if line.len() - t.len() != 12 || !t.starts_with('"') && !t.starts_with("| \"") {
                continue;
            }
            for name in t.split('"').skip(1).step_by(2) {
                if name.contains('.')
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
                {
                    out.push(name.to_string());
                }
            }
        }
        out
    }

    #[test]
    fn every_method_is_described_and_every_description_is_handled() {
        let handled = handled();
        assert!(handled.len() > 80, "found {} methods", handled.len());
        for h in &handled {
            assert!(find(h).is_some(), "{h} is handled but not in api.rs");
        }
        for m in METHODS {
            assert!(
                handled.iter().any(|h| h == m.name),
                "{} is described in api.rs but not handled",
                m.name
            );
        }
    }

    /// What the server takes while the app is away is model methods only,
    /// and real ones; the views' methods are the app's.
    #[test]
    fn methods_are_model_or_view() {
        for m in abromux_server::control::RECORDED
            .iter()
            .chain(abromux_server::control::ANSWERED)
            .chain(abromux_server::owned::OWNED)
            .chain(abromux_server::owned::READS)
        {
            assert!(find(m).is_some(), "{m} isn't a method");
            assert_eq!(kind(m), Kind::Model, "{m}");
        }
        for v in [
            "browser.click",
            "window.focus",
            "surface.send_text",
            "surface.read_text",
            "ui.prompt",
        ] {
            assert_eq!(kind(v), Kind::View, "{v}");
        }
        for m in [
            "workspace.create",
            "surface.split",
            "pane.create",
            "forward.add",
        ] {
            assert_eq!(kind(m), Kind::Model, "{m}");
        }
        let models = METHODS
            .iter()
            .filter(|m| kind(m.name) == Kind::Model)
            .count();
        assert!(
            models > 40 && models < METHODS.len() - 30,
            "{models} of {}",
            METHODS.len()
        );
        assert_eq!(describe(find("browser.click").unwrap())["kind"], "view");
    }

    #[test]
    fn describes_parameters_as_json_schema() {
        let v = describe(find("surface.split").unwrap());
        assert_eq!(v["cli"], "abromux new-split");
        let s = &v["input_schema"];
        assert_eq!(s["required"], json!(["direction"]));
        assert_eq!(s["properties"]["surface"]["type"], "string");
        assert!(
            s["properties"]["workspace"]["description"]
                .as_str()
                .unwrap()
                .contains("workspace:N")
        );
        assert_eq!(s["properties"]["label"]["type"], "string");
        assert_eq!(s["properties"]["focus"]["type"], "boolean");
        let text = describe_text(&v);
        assert!(text.contains("direction: string"), "{text}");
        assert!(methods(&json!({ "method": "nope" })).is_err());
    }
}
