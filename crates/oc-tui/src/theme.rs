//! Color themes. Theme files use the original OpenCode JSON format
//! (`defs` + `theme`, each color a hex string, a def/theme reference, an ANSI
//! number, or `{ "dark": .., "light": .. }`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use ratatui::style::{Color, Modifier, Style};
use serde_json::Value;

pub const DEFAULT_THEME: &str = "opencode";
pub const SYSTEM_THEME: &str = "system";

macro_rules! builtin {
    ($($name:literal),* $(,)?) => {
        const BUILTIN: &[(&str, &str)] = &[$(($name, include_str!(concat!("../themes/", $name, ".json")))),*];
    };
}
builtin!(
    "aura", "ayu", "carbonfox", "catppuccin", "catppuccin-frappe", "catppuccin-macchiato", "cobalt2", "cursor",
    "dracula", "everforest", "flexoki", "github", "gruvbox", "kanagawa", "lucent-orng", "material", "matrix",
    "mercury", "monokai", "nightowl", "nord", "one-dark", "opencode", "orng", "osaka-jade", "palenight", "rosepine",
    "solarized", "synthwave84", "tokyonight", "vercel", "vesper", "zenburn",
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    /// Best-effort terminal background detection via `COLORFGBG` (default dark).
    pub fn detect() -> Mode {
        match std::env::var("COLORFGBG").ok().and_then(|v| v.rsplit(';').next().and_then(|b| b.parse::<u8>().ok())) {
            Some(bg) if bg == 7 || bg == 15 => Mode::Light,
            _ => Mode::Dark,
        }
    }
}

/// Semantic color roles.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub text: Color,
    pub muted: Color,
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub info: Color,
    pub background: Color,
    pub panel: Color,
    pub element: Color,
    pub border: Color,
    pub border_active: Color,
    pub border_subtle: Color,
    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_added_bg: Color,
    pub diff_removed_bg: Color,
    pub md_heading: Color,
    pub md_code: Color,
    pub md_code_block: Color,
    pub md_link: Color,
    pub md_strong: Color,
    pub md_emph: Color,
    pub md_quote: Color,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub colors: Palette,
    pub text: Style,
    pub muted: Style,
    pub accent: Style,
    pub primary: Style,
    pub user: Style,
    pub heading: Style,
    pub bold: Style,
    pub code_inline: Style,
    pub code_block: Style,
    pub link: Style,
    pub tool: Style,
    pub tool_name: Style,
    pub error: Style,
    pub warning: Style,
    pub success: Style,
    pub status_bar: Style,
    pub border: Style,
    pub border_busy: Style,
    pub selected: Style,
    /// Fills the whole screen.
    pub base: Style,
    pub panel: Style,
    pub diff_add: Style,
    pub diff_remove: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::builtin(DEFAULT_THEME, Mode::Dark).expect("builtin opencode theme")
    }
}

impl Theme {
    /// Terminal-default colors only.
    pub fn system() -> Theme {
        let p = Palette {
            text: Color::Reset,
            muted: Color::DarkGray,
            primary: Color::Cyan,
            secondary: Color::Blue,
            accent: Color::Magenta,
            error: Color::Red,
            warning: Color::Yellow,
            success: Color::Green,
            info: Color::Cyan,
            background: Color::Reset,
            panel: Color::Reset,
            element: Color::Reset,
            border: Color::DarkGray,
            border_active: Color::Cyan,
            border_subtle: Color::DarkGray,
            diff_added: Color::Green,
            diff_removed: Color::Red,
            diff_added_bg: Color::Reset,
            diff_removed_bg: Color::Reset,
            md_heading: Color::Magenta,
            md_code: Color::Green,
            md_code_block: Color::Reset,
            md_link: Color::Cyan,
            md_strong: Color::Reset,
            md_emph: Color::Reset,
            md_quote: Color::DarkGray,
        };
        Theme::from_palette(SYSTEM_THEME, p)
    }

    pub fn builtin(name: &str, mode: Mode) -> anyhow::Result<Theme> {
        if name == SYSTEM_THEME {
            return Ok(Theme::system());
        }
        let (_, src) = BUILTIN.iter().find(|(n, _)| *n == name).ok_or_else(|| anyhow!("unknown builtin theme {name}"))?;
        Theme::from_json_str(name, src, mode)
    }

    pub fn from_json_str(name: &str, src: &str, mode: Mode) -> anyhow::Result<Theme> {
        let v: Value = serde_json::from_str(src).with_context(|| format!("theme {name}: invalid JSON"))?;
        Theme::from_json(name, &v, mode)
    }

    pub fn from_json(name: &str, json: &Value, mode: Mode) -> anyhow::Result<Theme> {
        let theme = json.get("theme").and_then(Value::as_object).ok_or_else(|| anyhow!("theme {name}: missing \"theme\" object"))?;
        let empty = serde_json::Map::new();
        let defs = json.get("defs").and_then(Value::as_object).unwrap_or(&empty);
        let r = Resolver { defs, theme, mode };
        let req = |k: &str| -> anyhow::Result<Color> {
            let v = theme.get(k).ok_or_else(|| anyhow!("theme {name}: missing color \"{k}\""))?;
            r.resolve(v, &mut Vec::new()).with_context(|| format!("theme {name}: color \"{k}\""))
        };
        let opt = |k: &str, fallback: Color| -> anyhow::Result<Color> {
            match theme.get(k) {
                Some(v) => r.resolve(v, &mut Vec::new()).with_context(|| format!("theme {name}: color \"{k}\"")),
                None => Ok(fallback),
            }
        };
        let text = req("text")?;
        let primary = req("primary")?;
        let background = req("background")?;
        let panel = opt("backgroundPanel", background)?;
        let accent = opt("accent", primary)?;
        let error = req("error")?;
        let success = req("success")?;
        let muted = opt("textMuted", text)?;
        let border = opt("border", muted)?;
        let p = Palette {
            text,
            muted,
            primary,
            secondary: opt("secondary", primary)?,
            accent,
            error,
            warning: opt("warning", error)?,
            success,
            info: opt("info", primary)?,
            background,
            panel,
            element: opt("backgroundElement", panel)?,
            border,
            border_active: opt("borderActive", primary)?,
            border_subtle: opt("borderSubtle", border)?,
            diff_added: opt("diffAdded", success)?,
            diff_removed: opt("diffRemoved", error)?,
            diff_added_bg: opt("diffAddedBg", Color::Reset)?,
            diff_removed_bg: opt("diffRemovedBg", Color::Reset)?,
            md_heading: opt("markdownHeading", accent)?,
            md_code: opt("markdownCode", success)?,
            md_code_block: opt("markdownCodeBlock", text)?,
            md_link: opt("markdownLink", primary)?,
            md_strong: opt("markdownStrong", text)?,
            md_emph: opt("markdownEmph", text)?,
            md_quote: opt("markdownBlockQuote", muted)?,
        };
        Ok(Theme::from_palette(name, p))
    }

    pub fn from_palette(name: &str, p: Palette) -> Theme {
        let fg = |c: Color| Style::default().fg(c);
        let sel_fg = if p.background == Color::Reset { Color::Black } else { p.background };
        let mut status = fg(p.text);
        if p.panel != Color::Reset {
            status = status.bg(p.panel);
        }
        let mut base = fg(p.text);
        if p.background != Color::Reset {
            base = base.bg(p.background);
        }
        let mut panel = fg(p.text);
        if p.panel != Color::Reset {
            panel = panel.bg(p.panel);
        }
        let diff = |c: Color, bg: Color| {
            let s = fg(c);
            if bg == Color::Reset {
                s
            } else {
                s.bg(bg)
            }
        };
        Theme {
            name: name.to_string(),
            text: fg(p.text),
            muted: fg(p.muted),
            accent: fg(p.accent),
            primary: fg(p.primary),
            user: fg(p.secondary).add_modifier(Modifier::BOLD),
            heading: fg(p.md_heading).add_modifier(Modifier::BOLD),
            bold: fg(p.md_strong).add_modifier(Modifier::BOLD),
            code_inline: fg(p.md_code),
            code_block: fg(p.md_code_block),
            link: fg(p.md_link).add_modifier(Modifier::UNDERLINED),
            tool: fg(p.muted),
            tool_name: fg(p.secondary),
            error: fg(p.error),
            warning: fg(p.warning),
            success: fg(p.success),
            status_bar: status,
            border: fg(p.border),
            border_busy: fg(p.border_active),
            selected: Style::default().fg(sel_fg).bg(p.primary),
            base,
            panel,
            diff_add: diff(p.diff_added, p.diff_added_bg),
            diff_remove: diff(p.diff_removed, p.diff_removed_bg),
            colors: p,
        }
    }
}

struct Resolver<'a> {
    defs: &'a serde_json::Map<String, Value>,
    theme: &'a serde_json::Map<String, Value>,
    mode: Mode,
}

impl Resolver<'_> {
    fn resolve(&self, v: &Value, chain: &mut Vec<String>) -> anyhow::Result<Color> {
        match v {
            Value::String(s) => {
                if s == "transparent" || s == "none" {
                    return Ok(Color::Reset);
                }
                if s.starts_with('#') {
                    return parse_hex(s);
                }
                if chain.contains(s) {
                    bail!("circular color reference: {} -> {s}", chain.join(" -> "));
                }
                let next = self
                    .defs
                    .get(s)
                    .or_else(|| self.theme.get(s))
                    .ok_or_else(|| anyhow!("color reference \"{s}\" not found in defs or theme"))?;
                chain.push(s.clone());
                let r = self.resolve(next, chain);
                chain.pop();
                r
            }
            Value::Number(n) => {
                let n = n.as_u64().ok_or_else(|| anyhow!("bad ansi color {n}"))?;
                Ok(Color::Indexed(n.min(255) as u8))
            }
            Value::Object(o) => {
                let key = if self.mode == Mode::Dark { "dark" } else { "light" };
                let inner = o.get(key).ok_or_else(|| anyhow!("color object missing \"{key}\""))?;
                self.resolve(inner, chain)
            }
            other => bail!("unsupported color value {other}"),
        }
    }
}

fn parse_hex(s: &str) -> anyhow::Result<Color> {
    let h = s.trim_start_matches('#');
    let full: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 => h.to_string(),
        8 => h[..6].to_string(), // alpha ignored
        _ => bail!("bad hex color {s}"),
    };
    let n = u32::from_str_radix(&full, 16).map_err(|_| anyhow!("bad hex color {s}"))?;
    Ok(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// All theme sources by name: builtins, then user dirs (later wins).
pub struct ThemeCatalog {
    custom: BTreeMap<String, Value>,
}

impl ThemeCatalog {
    pub fn load(cwd: &Path) -> ThemeCatalog {
        let mut dirs = Vec::new();
        if let Some(c) = config_dir() {
            dirs.push(c.join("opencode/themes"));
        }
        dirs.push(cwd.join(".opencode/themes"));
        ThemeCatalog::load_dirs(&dirs)
    }

    pub fn load_dirs(dirs: &[PathBuf]) -> ThemeCatalog {
        let mut custom = BTreeMap::new();
        for d in dirs {
            let Ok(rd) = std::fs::read_dir(d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
                if let Ok(txt) = std::fs::read_to_string(&p) {
                    if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                        custom.insert(stem.to_string(), v);
                    }
                }
            }
        }
        ThemeCatalog { custom }
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
        v.push(SYSTEM_THEME.into());
        for k in self.custom.keys() {
            if !v.contains(k) {
                v.push(k.clone());
            }
        }
        v.sort();
        v
    }

    pub fn get(&self, name: &str, mode: Mode) -> anyhow::Result<Theme> {
        if let Some(v) = self.custom.get(name) {
            return Theme::from_json(name, v, mode);
        }
        Theme::builtin(name, mode)
    }

    /// Falls back to the default theme when `name` is missing or invalid.
    pub fn get_or_default(&self, name: &str, mode: Mode) -> Theme {
        self.get(name, mode).unwrap_or_default()
    }
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".config")))
}

fn tui_state_path() -> PathBuf {
    oc_core::auth::data_dir().join("tui.json")
}

/// The theme remembered from a previous `/theme` selection.
pub fn load_saved_theme() -> Option<String> {
    let txt = std::fs::read_to_string(tui_state_path()).ok()?;
    let v: Value = serde_json::from_str(&txt).ok()?;
    v.get("theme")?.as_str().map(str::to_string)
}

pub fn save_theme(name: &str) {
    let path = tui_state_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Object(Default::default()));
    if let Some(o) = v.as_object_mut() {
        o.insert("theme".into(), Value::String(name.to_string()));
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_builtin_loads_in_both_modes() {
        for (name, _) in BUILTIN {
            for mode in [Mode::Dark, Mode::Light] {
                Theme::builtin(name, mode).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            }
        }
        for n in ["opencode", "tokyonight", "catppuccin", "gruvbox", "nord", "dracula", "one-dark", "everforest", "kanagawa", "matrix"] {
            assert!(BUILTIN.iter().any(|(b, _)| *b == n), "{n}");
        }
        assert_eq!(Theme::builtin("system", Mode::Dark).unwrap().colors.text, Color::Reset);
    }

    #[test]
    fn opencode_roles_resolve() {
        let d = Theme::builtin("opencode", Mode::Dark).unwrap();
        assert_eq!(d.colors.primary, Color::Rgb(0xfa, 0xb2, 0x83));
        assert_eq!(d.colors.error, Color::Rgb(0xe0, 0x6c, 0x75));
        let l = Theme::builtin("opencode", Mode::Light).unwrap();
        assert_eq!(l.colors.primary, Color::Rgb(0x3b, 0x7d, 0xd8));
        assert_eq!(l.error.fg, Some(Color::Rgb(0xd1, 0x38, 0x3d)));
    }

    #[test]
    fn defs_refs_variants_and_ansi() {
        let j = json!({
            "defs": {"a": "#112233", "b": "a", "c": {"dark": "#000000", "light": "#ffffff"}},
            "theme": {
                "text": "b", "primary": {"dark": "c", "light": "a"}, "background": "transparent",
                "error": 9, "success": "#0f0", "accent": "primary"
            }
        });
        let t = Theme::from_json("x", &j, Mode::Dark).unwrap();
        assert_eq!(t.colors.text, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(t.colors.primary, Color::Rgb(0, 0, 0));
        assert_eq!(t.colors.accent, Color::Rgb(0, 0, 0));
        assert_eq!(t.colors.background, Color::Reset);
        assert_eq!(t.colors.error, Color::Indexed(9));
        assert_eq!(t.colors.success, Color::Rgb(0, 0xff, 0));
        let t = Theme::from_json("x", &j, Mode::Light).unwrap();
        assert_eq!(t.colors.primary, Color::Rgb(0x11, 0x22, 0x33));
    }

    #[test]
    fn errors() {
        let circ = json!({"defs": {"a": "b", "b": "a"}, "theme": {"text": "a"}});
        assert!(format!("{:#}", Theme::from_json("x", &circ, Mode::Dark).unwrap_err()).contains("circular"));
        let missing = json!({"theme": {"text": "nope"}});
        assert!(Theme::from_json("x", &missing, Mode::Dark).is_err());
        assert!(Theme::from_json("x", &json!({}), Mode::Dark).is_err());
    }

    #[test]
    fn custom_dir_overrides_builtin() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mine.json"),
            r##"{"theme":{"text":"#010203","primary":"#040506","background":"#000000","error":"#ff0000","success":"#00ff00"}}"##,
        )
        .unwrap();
        std::fs::write(dir.path().join("nord.json"), r##"{"theme":{"text":"#111111","primary":"#222222","background":"#000000","error":"#ff0000","success":"#00ff00"}}"##).unwrap();
        std::fs::write(dir.path().join("broken.json"), "{").unwrap();
        let cat = ThemeCatalog::load_dirs(&[dir.path().to_path_buf()]);
        assert!(cat.names().contains(&"mine".to_string()));
        assert_eq!(cat.get("mine", Mode::Dark).unwrap().colors.text, Color::Rgb(1, 2, 3));
        assert_eq!(cat.get("nord", Mode::Dark).unwrap().colors.text, Color::Rgb(0x11, 0x11, 0x11));
        assert_eq!(cat.get_or_default("nonexistent", Mode::Dark).name, "opencode");
    }
}
