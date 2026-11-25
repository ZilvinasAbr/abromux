//! Settings declared rather than built in: a plugin's (its section of
//! config.json, e.g. `"gitlab": {…}`) and init.lua's (`abromux.setting`,
//! under `"scripts"`). abromux checks the file's values against them and
//! draws them in its Settings window; the plugin reads its own with
//! [`section`].

use gpui::{App, Global};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// One setting.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingDef {
    pub key: String,
    pub title: String,
    pub description: Option<String>,
    /// The group it's listed under on its page.
    pub group: Option<String>,
    pub kind: SettingKind,
    pub default: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SettingKind {
    Boolean,
    Number {
        min: f64,
        max: f64,
        step: f64,
        /// Shown after the value, e.g. `MB`.
        unit: Option<String>,
    },
    /// `(value, label)`s.
    Choice(Vec<(String, String)>),
    /// Free text; its default is `null` (none). Saved when the field is left.
    Text {
        placeholder: Option<String>,
    },
}

impl SettingDef {
    /// `v`, if it's a value this setting can take.
    pub fn accepts(&self, v: &Value) -> bool {
        match &self.kind {
            SettingKind::Boolean => v.is_boolean(),
            SettingKind::Number { min, max, .. } => {
                v.as_f64().is_some_and(|n| n >= *min && n <= *max)
            }
            SettingKind::Choice(options) => v
                .as_str()
                .is_some_and(|s| options.iter().any(|(o, _)| o == s)),
            SettingKind::Text { .. } => v.is_string() || v.is_null(),
        }
    }

    pub fn boolean(key: &str, title: &str, default: bool) -> Self {
        Self::new(key, title, SettingKind::Boolean, Value::Bool(default))
    }

    pub fn number(key: &str, title: &str, default: f64, (min, max, step): (f64, f64, f64)) -> Self {
        let kind = SettingKind::Number {
            min,
            max,
            step,
            unit: None,
        };
        let default = if default.fract() == 0. {
            Value::from(default as i64)
        } else {
            Value::from(default)
        };
        Self::new(key, title, kind, default)
    }

    pub fn choice(key: &str, title: &str, options: &[(&str, &str)], default: &str) -> Self {
        let options = options
            .iter()
            .map(|(v, l)| (v.to_string(), l.to_string()))
            .collect();
        Self::new(key, title, SettingKind::Choice(options), default.into())
    }

    pub fn text(key: &str, title: &str, placeholder: &str) -> Self {
        let kind = SettingKind::Text {
            placeholder: Some(placeholder.into()),
        };
        Self::new(key, title, kind, Value::Null)
    }

    fn new(key: &str, title: &str, kind: SettingKind, default: Value) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            description: None,
            group: None,
            kind,
            default,
        }
    }

    pub fn description(mut self, d: &str) -> Self {
        self.description = Some(d.into());
        self
    }

    pub fn group(mut self, g: &str) -> Self {
        self.group = Some(g.into());
        self
    }

    /// A number's unit.
    pub fn unit(mut self, u: &str) -> Self {
        if let SettingKind::Number { unit, .. } = &mut self.kind {
            *unit = Some(u.into());
        }
        self
    }
}

/// A plugin's page in the Settings window, and its section of config.json
/// (the plugin's id: `"gitlab": {…}`).
pub struct SettingsPage {
    pub title: &'static str,
    /// One of abromux's `assets/icons`, e.g. `icons/git-pull-request.svg`.
    pub icon: &'static str,
    pub description: Option<&'static str>,
    pub settings: Vec<SettingDef>,
}

impl SettingsPage {
    /// What's wrong with `section` (its value in config.json), if anything:
    /// a key nothing declares, or a value its setting doesn't take.
    pub fn check(&self, id: &str, section: &Value) -> Result<(), String> {
        let map = match section {
            Value::Null => return Ok(()),
            Value::Object(m) => m,
            _ => return Err(format!("{id}: expected an object")),
        };
        for (k, v) in map {
            let Some(def) = self.settings.iter().find(|d| d.key == *k) else {
                let known: Vec<String> = self
                    .settings
                    .iter()
                    .map(|d| format!("`{}`", d.key))
                    .collect();
                return Err(format!(
                    "{id}: unknown field `{k}`, expected one of {}",
                    known.join(", ")
                ));
            };
            if !def.accepts(v) {
                return Err(format!("{id}.{k}: {v} isn't a value it takes"));
            }
        }
        Ok(())
    }

    /// `section` with every declared setting it leaves out at its default.
    pub fn filled(&self, section: &Value) -> Value {
        let mut map = section.as_object().cloned().unwrap_or_default();
        for d in &self.settings {
            if !map.contains_key(&d.key) {
                map.insert(d.key.clone(), d.default.clone());
            }
        }
        Value::Object(map)
    }
}

/// Each plugin's section of config.json, as abromux last loaded it (and
/// checked). Set by abromux; a plugin reads its own with [`section`].
#[derive(Default)]
pub struct Sections(pub BTreeMap<String, Value>);

impl Global for Sections {}

/// Plugin `id`'s section of config.json: what the file sets, the rest at
/// the declared defaults (an empty object before abromux sets any).
pub fn section(id: &str, cx: &App) -> Value {
    cx.try_global::<Sections>()
        .and_then(|s| s.0.get(id).cloned())
        .unwrap_or_else(|| Value::Object(Map::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page() -> SettingsPage {
        SettingsPage {
            title: "T",
            icon: "",
            description: None,
            settings: vec![
                SettingDef::boolean("on", "On", true),
                SettingDef::number("n", "N", 5., (1., 10., 1.)),
                SettingDef::choice("c", "C", &[("a", "A"), ("b", "B")], "a"),
                SettingDef::text("t", "T", "none"),
            ],
        }
    }

    #[test]
    fn check() {
        let p = page();
        assert!(p.check("x", &Value::Null).is_ok());
        assert!(
            p.check("x", &json!({"on": false, "n": 3, "c": "b", "t": "hi"}))
                .is_ok()
        );
        assert!(p.check("x", &json!({"t": null})).is_ok());
        let e = p.check("x", &json!({"nope": 1})).unwrap_err();
        assert!(
            e.contains("unknown field `nope`") && e.contains("`on`"),
            "{e}"
        );
        assert!(p.check("x", &json!({"n": 11})).unwrap_err().contains("x.n"));
        assert!(p.check("x", &json!({"c": "z"})).is_err());
        assert!(p.check("x", &json!([])).is_err());
    }

    #[test]
    fn filled() {
        let v = page().filled(&json!({"n": 3}));
        assert_eq!(v, json!({"n": 3, "on": true, "c": "a", "t": null}));
    }
}
