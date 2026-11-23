// Settings live in %APPDATA%\notch\config.toml. The defaults are the commented file itself, so they're defined once.
use std::path::PathBuf;

use serde::Deserialize;

pub const DEFAULT: &str = include_str!("default_config.toml");

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub hotkey: String,
    pub theme: Theme,
    pub focus: Focus,
    pub prayer: Prayer,
    pub jot: Jot,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Focus {
    pub focus: f32,
    pub short_break: f32,
    pub long_break: f32,
    pub rounds: u32,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Prayer {
    pub latitude: f64,
    pub longitude: f64,
    pub fajr_angle: f64,
    pub isha_angle: f64,
    pub ihtiyat: i32,
    pub madhab: String,
    pub names: Vec<String>,
    pub pause_music: bool,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Jot {
    pub hotkey: String,
    pub file: String,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub background: String,
    pub text: String,
    pub font: String,
    pub closed_width: f32,
    pub closed_height: f32,
    pub open_width: f32,
    pub open_height: f32,
    pub radius: f32,
}

impl Default for Config {
    fn default() -> Self {
        toml::from_str(DEFAULT).expect("default_config.toml is valid")
    }
}

impl Config {
    /// Parses the user's file over the defaults, so a file with only some keys still works.
    pub fn parse(user: &str) -> Result<Self, String> {
        let mut base: toml::Table = DEFAULT.parse().map_err(|e| format!("{e}"))?;
        let user: toml::Table = user.parse().map_err(|e: toml::de::Error| e.message().to_string())?;
        for (k, v) in user {
            match (base.get_mut(&k), v) {
                (Some(toml::Value::Table(b)), toml::Value::Table(u)) => b.extend(u),
                (_, v) => {
                    base.insert(k, v);
                }
            }
        }
        let c: Config = base.try_into().map_err(|e: toml::de::Error| e.message().to_string())?;
        let t = &c.theme;
        for (name, v) in [("background", &t.background), ("text", &t.text)] {
            parse_color(v).ok_or(format!("theme.{name}: \"{v}\" isn't a #RRGGBB colour"))?;
        }
        for (name, v) in [("hotkey", &c.hotkey), ("jot.hotkey", &c.jot.hotkey)] {
            parse_hotkey(v).ok_or(format!("{name}: \"{v}\" isn't a key combination"))?;
        }
        if c.hotkey.eq_ignore_ascii_case(&c.jot.hotkey) {
            return Err("hotkey and jot.hotkey are the same keys".into());
        }
        let p = &c.prayer;
        if !(-90.0..=90.0).contains(&p.latitude) || !(-180.0..=180.0).contains(&p.longitude) {
            return Err("prayer.latitude must be -90..90 and prayer.longitude -180..180".into());
        }
        if p.madhab != "shafi" && p.madhab != "hanafi" {
            return Err(format!("prayer.madhab: \"{}\" should be \"shafi\" or \"hanafi\"", p.madhab));
        }
        if p.names.len() != 5 {
            return Err("prayer.names needs five names, Fajr to Isha".into());
        }
        Ok(c)
    }

    /// Reads the file, writing the commented defaults on first run. Errors come back as text to show in the notch.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = path() else { return (Self::default(), None) };
        match std::fs::read_to_string(&path) {
            Ok(text) => match Self::parse(&text) {
                Ok(c) => (c, None),
                Err(e) => (Self::default(), Some(format!("config.toml: {e}"))),
            },
            Err(_) => {
                let _ = std::fs::create_dir_all(path.parent().unwrap());
                let _ = std::fs::write(&path, DEFAULT);
                (Self::default(), None)
            }
        }
    }
}

pub fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("notch").join("config.toml"))
}

/// "#RRGGBB" or "#RRGGBBAA" → [r, g, b, a] in 0..1.
pub fn parse_color(s: &str) -> Option<[f32; 4]> {
    let hex = s.strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|b| b as f32 / 255.0);
    Some([byte(0)?, byte(2)?, byte(4)?, if hex.len() == 8 { byte(6)? } else { 1.0 }])
}

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

/// "Ctrl+Alt+N" → (modifier flags for RegisterHotKey, virtual-key code).
pub fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    let mut mods = 0;
    let mut key = None;
    for part in s.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" => mods |= MOD_WIN,
            "space" => key = Some(0x20),
            k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => key = Some(k.to_ascii_uppercase().as_bytes()[0] as u32),
            k => match k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
                Some(n @ 1..=12) => key = Some(0x70 + n - 1),
                _ => return None,
            },
        }
    }
    // A bare letter would steal normal typing.
    if mods == 0 || mods == MOD_SHIFT { None } else { Some((mods, key?)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse() {
        let c = Config::default();
        assert_eq!(c.hotkey, "Ctrl+Alt+N");
        assert_eq!(c.theme.open_width, 400.0);
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let c = Config::parse("[theme]\nbackground = \"#101820\"\n").unwrap();
        assert_eq!(c.theme.background, "#101820");
        assert_eq!(c.theme.text, "#FFFFFF");
        assert_eq!(c.hotkey, "Ctrl+Alt+N");
    }

    #[test]
    fn mistakes_are_explained() {
        assert!(Config::parse("[theme]\nbackgrund = \"#000\"").unwrap_err().contains("backgrund"));
        assert!(Config::parse("[theme]\ntext = \"white\"").unwrap_err().contains("theme.text"));
        assert!(Config::parse("hotkey = \"N\"").unwrap_err().contains("hotkey"));
        assert!(Config::parse("hotkey = ").is_err());
        assert!(Config::parse("[prayer]\nmadhab = \"maliki\"").unwrap_err().contains("madhab"));
        assert!(Config::parse("[prayer]\nnames = [\"Subuh\"]").unwrap_err().contains("five"));
        assert!(Config::parse("[prayer]\nlatitude = 120.0").unwrap_err().contains("latitude"));
        assert!(Config::parse("[jot]\nhotkey = \"Ctrl+Alt+N\"").unwrap_err().contains("same keys"));
    }

    #[test]
    fn colours() {
        assert_eq!(parse_color("#FF0000"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_color("#00000080").map(|c| (c[3] * 255.0).round()), Some(128.0));
        assert_eq!(parse_color("FF0000"), None);
        assert_eq!(parse_color("#FF00"), None);
        assert_eq!(parse_color("#GG0000"), None);
    }

    #[test]
    fn hotkeys() {
        assert_eq!(parse_hotkey("Ctrl+Alt+N"), Some((MOD_CONTROL | MOD_ALT, 'N' as u32)));
        assert_eq!(parse_hotkey("win + space"), Some((MOD_WIN, 0x20)));
        assert_eq!(parse_hotkey("Ctrl+Shift+F9"), Some((MOD_CONTROL | MOD_SHIFT, 0x78)));
        assert_eq!(parse_hotkey("Ctrl+F13"), None);
        assert_eq!(parse_hotkey("Shift+A"), None);
        assert_eq!(parse_hotkey("Ctrl+Alt"), None);
    }
}
