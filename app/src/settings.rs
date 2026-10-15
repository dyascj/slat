//! `settings.json`: the user's choices. Strict like the original: any missing
//! key, wrong type or unknown value discards the whole file for defaults.

use serde::{Deserialize, Serialize};
use std::fs;

use crate::paths;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    Columns,
    Canvas,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Padding {
    None,
    Comfy,
    Spacious,
}

impl Padding {
    pub const ALL: [Padding; 3] = [Padding::None, Padding::Comfy, Padding::Spacious];
    pub fn points(self) -> i32 {
        self as i32 * 8
    }
    pub fn raw(self) -> &'static str {
        ["none", "comfy", "spacious"][self as usize]
    }
    pub fn label(self) -> &'static str {
        ["None", "Comfy", "Spacious"][self as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Animation {
    Off,
    Smooth,
    Snappy,
}

impl Animation {
    pub const ALL: [Animation; 3] = [Animation::Off, Animation::Smooth, Animation::Snappy];
    pub fn engine_value(self) -> Option<f64> {
        [None, Some(10.0), Some(18.0)][self as usize]
    }
    pub fn raw(self) -> &'static str {
        ["off", "smooth", "snappy"][self as usize]
    }
    pub fn label(self) -> &'static str {
        ["Off", "Smooth", "Snappy"][self as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwipeDirection {
    Natural,
    Reversed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModKey {
    CtrlAlt,
    Alt,
    CmdAlt,
}

impl ModKey {
    /// Order shown in the UI chips.
    pub const ALL: [ModKey; 3] = [ModKey::CtrlAlt, ModKey::Alt, ModKey::CmdAlt];
    pub fn engine_modifier(self) -> &'static str {
        match self {
            ModKey::Alt => "alt",
            ModKey::CtrlAlt => "ctrl + alt",
            ModKey::CmdAlt => "cmd + alt",
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            ModKey::Alt => "⌥",
            ModKey::CtrlAlt => "⌃⌥",
            ModKey::CmdAlt => "⌘⌥",
        }
    }
    pub fn raw(self) -> &'static str {
        match self {
            ModKey::Alt => "alt",
            ModKey::CtrlAlt => "ctrlAlt",
            ModKey::CmdAlt => "cmdAlt",
        }
    }
}

/// Swift-synthesized enum coding, kept for file compatibility:
/// `{"float": {}}` or `{"width": {"_0": 0.5}}`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Float {},
    Width {
        #[serde(rename = "_0")]
        w: f64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRule {
    pub bundle_id: String,
    pub app_name: String,
    pub action: RuleAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub focus_follows_mouse: bool,
    pub mouse_follows_focus: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
    pub preset_column_widths: Vec<f64>,
    pub default_column_width: f64,
    pub auto_center: bool,
    pub padding: Padding,
    pub animation: Animation,
    pub swipe_enabled: bool,
    pub swipe_fingers: i64,
    pub swipe_direction: SwipeDirection,
    pub swipe_sensitivity: f64,
    pub scroll_modifier: String,
    pub virtual_workspaces_enabled: bool,
    pub virtual_workspace_count: i64,
    pub create_workspaces_automatically: bool,
    pub dim_inactive_windows: bool,
    pub focus_border_enabled: bool,
    pub mod_key: ModKey,
    pub start_at_login: bool,
    pub app_rules: Vec<AppRule>,
    pub use_custom_config_file: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            focus_follows_mouse: false,
            mouse_follows_focus: false,
            layout: None,
            preset_column_widths: vec![0.5, 0.66, 1.0],
            default_column_width: 0.5,
            auto_center: false,
            padding: Padding::Comfy,
            animation: Animation::Smooth,
            swipe_enabled: true,
            swipe_fingers: 4,
            swipe_direction: SwipeDirection::Natural,
            swipe_sensitivity: 0.35,
            scroll_modifier: "alt".into(),
            virtual_workspaces_enabled: true,
            virtual_workspace_count: 4,
            create_workspaces_automatically: false,
            dim_inactive_windows: false,
            focus_border_enabled: false,
            mod_key: ModKey::CtrlAlt,
            start_at_login: true,
            app_rules: vec![],
            use_custom_config_file: false,
        }
    }
}

/// Width values offered by every width picker.
pub const WIDTHS: [f64; 6] = [0.25, 0.33, 0.5, 0.66, 0.75, 1.0];

pub fn width_label(w: f64) -> &'static str {
    match WIDTHS.iter().position(|x| (x - w).abs() < 0.005) {
        Some(0) => "¼",
        Some(1) => "⅓",
        Some(2) => "½",
        Some(3) => "⅔",
        Some(4) => "¾",
        _ => "Full",
    }
}

impl Settings {
    pub fn layout(&self) -> Layout {
        self.layout.unwrap_or_default()
    }

    pub fn parse(text: &str) -> Settings {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn load() -> Settings {
        fs::read_to_string(paths::settings_file()).map_or_else(|_| Settings::default(), |t| Settings::parse(&t))
    }

    /// Pretty, key-sorted JSON (`"key" : value`, like the Foundation encoder).
    pub fn to_json(&self) -> String {
        // Foundation writes integral doubles without ".0" (1.0 -> 1).
        fn integralize(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Number(n) => {
                    if let Some(f) = n.as_f64().filter(|f| n.is_f64() && f.fract() == 0.0 && f.abs() < 1e15) {
                        #[allow(clippy::cast_possible_truncation)]
                        let i = f as i64;
                        *n = i.into();
                    }
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(integralize),
                serde_json::Value::Object(o) => o.values_mut().for_each(integralize),
                _ => {}
            }
        }
        let mut sorted: serde_json::Value = serde_json::to_value(self).expect("settings serialize");
        integralize(&mut sorted);
        let mut buf = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b"  ");
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
        sorted.serialize(&mut ser).expect("settings serialize");
        String::from_utf8(buf).expect("utf8").replace("\": ", "\" : ")
    }

    /// Writes settings.json and, unless custom config mode is on, engine.toml.
    pub fn save(&self) -> std::io::Result<()> {
        paths::ensure_dirs();
        fs::write(paths::settings_file(), self.to_json())?;
        crate::config::apply(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_parse_and_roundtrip() {
        let observed = include_str!("../tests/fixtures/settings.json");
        let s = Settings::parse(observed);
        assert_eq!(s.animation, Animation::Snappy);
        assert_eq!(s.mod_key, ModKey::Alt);
        assert_eq!(Settings::parse(&s.to_json()), s);
        assert!(s.to_json().contains("\"presetColumnWidths\" : [\n    0.5,\n    0.66,\n    1\n  ]"));
        // one missing key throws the whole file away
        assert_eq!(Settings::parse(&observed.replace("\"autoCenter\" : false,", "")), Settings::default());
        let rule = r#"{"bundleId":"a","appName":"A","action":{"width":{"_0":0.5}}}"#;
        let r: AppRule = serde_json::from_str(rule).unwrap();
        assert_eq!(r.action, RuleAction::Width { w: 0.5 });
        assert_eq!(serde_json::to_string(&RuleAction::Float {}).unwrap(), r#"{"float":{}}"#);
    }
}
