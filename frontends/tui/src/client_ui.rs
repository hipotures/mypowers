//! Local client appearance and navigation bindings; never sent to the daemon.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

pub const THEMES: [&str; 7] = [
    "MyPowers",
    "Catppuccin",
    "Nord",
    "Gruvbox",
    "Tokyo Night",
    "Solarized",
    "Terminal",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientPreferences {
    #[serde(default = "default_theme")]
    pub theme: String,
    pub colors: BTreeMap<String, String>,
    pub keybindings: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

impl Default for ClientPreferences {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            colors: BTreeMap::new(),
            keybindings: BTreeMap::new(),
            server_url: None,
            token_file: None,
            ca_file: None,
            timezone: None,
            path: None,
        }
    }
}

fn default_theme() -> String {
    "MyPowers".into()
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

impl ClientPreferences {
    pub fn load(path: PathBuf, required: bool) -> io::Result<Self> {
        let mut preferences = if path.exists() || required {
            // Parse errors omit file contents, which may contain private connection details.
            ::config::Config::builder()
                .add_source(::config::File::from(path.as_path()).format(::config::FileFormat::Toml))
                .build()
                .and_then(|config| config.try_deserialize::<Self>())
                .map_err(|_| invalid("Invalid client TOML configuration."))?
        } else {
            Self {
                theme: default_theme(),
                ..Self::default()
            }
        };
        preferences.validate()?;
        preferences.path = Some(path);
        Ok(preferences)
    }

    pub fn validate(&self) -> io::Result<()> {
        if !THEMES.contains(&self.theme.as_str()) {
            return Err(invalid("Unknown client theme."));
        }
        for (role, color) in &self.colors {
            if ![
                "background",
                "foreground",
                "muted",
                "border",
                "focus",
                "accent",
                "error",
                "warning",
            ]
            .contains(&role.as_str())
                || parse_color(color).is_none()
            {
                return Err(invalid("Use a supported color role and #RRGGBB."));
            }
        }
        for (mode, bindings) in &self.keybindings {
            if !["dashboard", "logs", "settings"].contains(&mode.as_str()) {
                return Err(invalid("Unknown keybinding view."));
            }
            for (key, action) in bindings {
                if parse_key(key).is_none() || action_key(action).is_none() {
                    return Err(invalid("Invalid client keybinding."));
                }
                // These keys remain reliable ways to leave screens and cancel dialogs.
                if matches!(parse_key(key), Some(KeyCode::Esc | KeyCode::Char('q'))) {
                    return Err(invalid("Escape and q are reserved."));
                }
            }
        }
        Ok(())
    }

    pub fn save_theme(&mut self, name: &str) -> io::Result<()> {
        let mut candidate = if let Some(path) = &self.path {
            if path.exists() {
                Self::load(path.clone(), true)?
            } else {
                self.clone()
            }
        } else {
            self.clone()
        };
        candidate.theme = name.into();
        candidate.validate()?;
        let path = candidate
            .path
            .as_ref()
            .ok_or_else(|| invalid("No client configuration path."))?;
        let parent = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        // A unique temporary file prevents concurrent clients from sharing a staging file.
        let temporary = parent.join(format!(".mypowers-{}.toml", uuid::Uuid::new_v4()));
        let contents = toml::to_string_pretty(&candidate)
            .map_err(|_| invalid("Cannot encode client configuration."))?;
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        *self = candidate;
        Ok(())
    }

    pub fn theme(&self) -> ratcn::Theme {
        let mut theme = match self.theme.as_str() {
            "Catppuccin" => ratcn::Theme::catppuccin(),
            "Nord" => ratcn::Theme::nord(),
            "Gruvbox" => ratcn::Theme::gruvbox(),
            "Tokyo Night" => ratcn::Theme::tokyo_night(),
            "Solarized" => ratcn::Theme::solarized(),
            "Terminal" => ratcn::Theme::terminal(),
            _ => {
                let mut t = ratcn::Theme::default_dark();
                t.background = Color::Rgb(16, 21, 27);
                t.surface = t.background;
                t.foreground = Color::Rgb(224, 232, 236);
                t.muted_foreground = Color::Rgb(119, 144, 153);
                t.border = Color::Rgb(68, 94, 105);
                t.field = Color::Rgb(34, 46, 54);
                t.secondary = t.field;
                t.secondary_foreground = t.foreground;
                t.primary = Color::Rgb(118, 203, 137);
                t.primary_foreground = t.background;
                t.accent = t.primary;
                t.ring = t.primary;
                t.destructive = Color::Rgb(229, 101, 111);
                t.warning = Color::Rgb(220, 199, 111);
                t
            }
        };
        for (role, value) in &self.colors {
            if let Some(color) = parse_color(value) {
                match role.as_str() {
                    "background" => {
                        if theme.primary_foreground == theme.background {
                            theme.primary_foreground = color;
                        }
                        if theme.destructive_foreground == theme.background {
                            theme.destructive_foreground = color;
                        }
                        theme.background = color;
                        theme.surface = color;
                    }
                    "foreground" => theme.foreground = color,
                    "muted" => theme.muted_foreground = color,
                    "border" => theme.border = color,
                    "focus" => {
                        theme.field = color;
                        theme.secondary = color;
                    }
                    "accent" => {
                        theme.primary = color;
                        theme.accent = color;
                        theme.ring = color;
                    }
                    "error" => theme.destructive = color,
                    "warning" => theme.warning = color,
                    _ => {}
                }
            }
        }
        theme
    }

    pub fn key(&self, mode: &str, key: KeyEvent) -> KeyEvent {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || key.code == KeyCode::BackTab
        {
            return key;
        }
        self.keybindings
            .get(mode)
            .and_then(|bindings| {
                bindings.iter().find_map(|(source, action)| {
                    (parse_key(source) == Some(key.code))
                        .then(|| action_key(action))
                        .flatten()
                })
            })
            .map(|code| KeyEvent::new(code, KeyModifiers::NONE))
            .unwrap_or(key)
    }
}

fn parse_color(text: &str) -> Option<Color> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(Color::Rgb(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ))
}
fn parse_key(text: &str) -> Option<KeyCode> {
    Some(match text {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Tab" => KeyCode::Tab,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "F1" => KeyCode::F(1),
        "F3" => KeyCode::F(3),
        value if value.chars().count() == 1 && !value.chars().next()?.is_control() => {
            KeyCode::Char(value.chars().next()?)
        }
        _ => return None,
    })
}
fn action_key(action: &str) -> Option<KeyCode> {
    Some(match action {
        "help" => KeyCode::F(1),
        "logs" => KeyCode::F(3),
        "settings" => KeyCode::Char('s'),
        "quit" => KeyCode::Char('q'),
        "close" => KeyCode::Esc,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "activate" => KeyCode::Enter,
        "next_tab" => KeyCode::Tab,
        "previous_tab" => KeyCode::BackTab,
        "cycle_interval" => KeyCode::Char('t'),
        "toggle_graph" => KeyCode::Char('g'),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("mypowers-client-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> PathBuf {
            self.0.join("client.toml")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn theme_save_preserves_other_preferences_and_new_edits() {
        let dir = Temp::new();
        fs::write(dir.path(), "theme = 'Nord'\nserver_url = 'https://example.net'\n[colors]\nborder = '#123456'\n[keybindings.dashboard]\nx = 'settings'\n").unwrap();
        let mut preferences = ClientPreferences::load(dir.path(), true).unwrap();
        let edited = fs::read_to_string(dir.path())
            .unwrap()
            .replace("example.net", "other.example.net");
        fs::write(dir.path(), edited).unwrap();
        preferences.save_theme("Catppuccin").unwrap();
        let saved = ClientPreferences::load(dir.path(), true).unwrap();
        assert_eq!(saved.theme, "Catppuccin");
        assert_eq!(
            saved.server_url.as_deref(),
            Some("https://other.example.net")
        );
        assert_eq!(saved.theme().border, Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(saved.keybindings["dashboard"]["x"], "settings");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[test]
    fn invalid_local_preferences_fail_without_leaking_contents() {
        let dir = Temp::new();
        for text in [
            "theme = 'unknown'",
            "[colors]\nborder = '#gg1234'",
            "[keybindings.settings]\nq = 'up'",
            "[keybindings.unknown]\nx = 'help'",
            "unexpected_secret = 'private-value'",
            "[keybindings.settings]\nx = 'reboot'",
        ] {
            fs::write(dir.path(), text).unwrap();
            let error = ClientPreferences::load(dir.path(), true)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("private-value"));
        }
        assert!(ClientPreferences::load(dir.0.join("missing.toml"), true).is_err());
        assert_eq!(
            ClientPreferences::load(dir.0.join("optional.toml"), false)
                .unwrap()
                .theme,
            "MyPowers"
        );
    }
    #[test]
    fn bindings_are_scoped_and_do_not_intercept_control_keys() {
        let mut preferences = ClientPreferences::default();
        preferences.keybindings.insert(
            "settings".into(),
            BTreeMap::from([("j".into(), "down".into())]),
        );
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(preferences.key("settings", key).code, KeyCode::Down);
        assert_eq!(preferences.key("dashboard", key), key);
        let control = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(preferences.key("settings", control), control);
    }
    #[test]
    fn toml_connection_defaults_yield_to_dotenv_and_cli() {
        let dir = Temp::new();
        fs::write(
            dir.path(),
            "server_url = 'https://example.net'\ntimezone = 'UTC'\n",
        )
        .unwrap();
        let env = dir.0.join("client.env");
        fs::write(&env, "MYPOWERS_SERVER_URL=https://dotenv.example.net\n").unwrap();
        let args = vec![
            "--client-config".into(),
            dir.path().to_string_lossy().into_owned(),
            "--env-file".into(),
            env.to_string_lossy().into_owned(),
        ];
        let cfg = crate::config::Config::parse(args.clone()).unwrap().unwrap();
        assert_eq!(cfg.server.host_str(), Some("dotenv.example.net"));
        let mut args = args;
        args.extend(["--server".into(), "https://cli.example.net".into()]);
        let cfg = crate::config::Config::parse(args).unwrap().unwrap();
        assert_eq!(cfg.server.host_str(), Some("cli.example.net"));
    }
}
