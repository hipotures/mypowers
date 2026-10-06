use std::{
    collections::HashMap, env, fs, io, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration,
};

pub struct Config {
    pub client_preferences: crate::client_ui::ClientPreferences,
    pub server: reqwest::Url,
    pub token: Option<String>,
    pub ca: Option<PathBuf>,
    pub timeout: Duration,
    pub no_color: bool,
    pub no_mouse: bool,
    pub timezone: Option<chrono_tz::Tz>,
}

fn error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

impl Config {
    pub fn load() -> io::Result<Option<Self>> {
        let arguments = env::args_os()
            .skip(1)
            .map(|value| {
                value
                    .into_string()
                    .map_err(|_| error("Arguments must use UTF-8."))
            })
            .collect::<io::Result<Vec<_>>>()?;
        Self::parse(arguments)
    }

    pub fn parse(args: Vec<String>) -> io::Result<Option<Self>> {
        let mut options = HashMap::new();
        let (mut no_color, mut no_mouse, mut utc) =
            (env::var_os("NO_COLOR").is_some(), false, false);
        let mut arguments = args.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--help" | "-h" => {
                    println!(
                        "MyPowers Ratatui client\n\nUsage: mypowers-tui [options]\n\n  --client-config PATH  Local client TOML (theme and shortcuts)\n  --env-file PATH   Read PATH instead of .env in the current directory\n  --server URL      HTTP(S) daemon origin\n  --token-file PATH Private API token file\n  --ca-file PATH    PEM CA bundle for HTTPS and WSS\n  --timeout SECS    Request timeout (0 < SECS <= 120)\n  --timezone ZONE   IANA timezone for log days/timestamps\n  --utc             UTC log timestamps\n  --no-color        Disable colors\n  --no-mouse        Disable mouse capture\n\nDashboard: a/d/l outputs; Tab, Enter focus/activate; F3 logs; s settings; t interval; g graph view\nLogs: Left/Right day; +/- page size; f filter; b runtime DEBUG\nLogs: Home day beginning; End fetch today/live\nSettings: Tab/Shift-Tab selects a tab; Up/Down selects a field or button\nEnter or click: open choices or confirm a value; autosave after 5s\nDebug: Retry, Pause/Resume and Debug ON/OFF buttons\nF1 / ?: contextual help from dashboard, logs, or settings\nGlobal: Esc close modal; q confirm quit\nDouble-click MYPOWERS / LOGS to copy the API snapshot / loaded logs (OSC 52 / wl-copy)."
                    );
                    return Ok(None);
                }
                "--no-color" => no_color = true,
                "--no-mouse" => no_mouse = true,
                "--utc" => utc = true,
                "--env-file" | "--server" | "--token-file" | "--ca-file" | "--timeout"
                | "--timezone" | "--client-config" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| error("Missing option value. Use --help."))?;
                    options.insert(argument, value);
                }
                _ => return Err(error("Unknown argument. Use --help.")),
            }
        }
        let explicit = options.get("--env-file");
        let path = PathBuf::from(explicit.map(String::as_str).unwrap_or(".env"));
        let explicit_client = options
            .get("--client-config")
            .cloned()
            .or_else(|| env::var("MYPOWERS_CLIENT_CONFIG").ok());
        let default_dir = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or_else(|| error("Cannot find client configuration directory."))?;
        let client_path = explicit_client
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| default_dir.join("mypowers/client.toml"));
        let client_path = if client_path.is_absolute() {
            client_path
        } else {
            env::current_dir()?.join(client_path)
        };
        let client_preferences =
            crate::client_ui::ClientPreferences::load(client_path, explicit_client.is_some())?;
        let mut values = HashMap::new();
        for (key, value) in [
            ("MYPOWERS_SERVER_URL", &client_preferences.server_url),
            ("MYPOWERS_API_TOKEN_FILE", &client_preferences.token_file),
            ("MYPOWERS_CA_FILE", &client_preferences.ca_file),
            ("MYPOWERS_TIMEZONE", &client_preferences.timezone),
        ] {
            if let Some(value) = value {
                let value = if matches!(key, "MYPOWERS_API_TOKEN_FILE" | "MYPOWERS_CA_FILE") {
                    client_preferences
                        .path
                        .as_ref()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join(value)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    value.clone()
                };
                values.insert(key.to_owned(), value);
            }
        }
        if explicit.is_some() || path.exists() {
            let path =
                fs::canonicalize(path).map_err(|_| error("Cannot read selected env file."))?;
            // Load into a private map; never mutate the process environment.
            let contents = fs::read_to_string(&path)?;
            for item in dotenvy::from_read_iter(contents.as_bytes()) {
                let (key, mut value) = item.map_err(|_| error("Invalid dotenv syntax."))?;
                if matches!(key.as_str(), "MYPOWERS_API_TOKEN_FILE" | "MYPOWERS_CA_FILE") {
                    value = path
                        .parent()
                        .unwrap()
                        .join(value)
                        .to_string_lossy()
                        .into_owned();
                }
                if key.starts_with("MYPOWERS_") {
                    values.insert(key, value);
                }
            }
        }
        for (key, value) in env::vars_os() {
            if let Some(key) = key.to_str().filter(|key| key.starts_with("MYPOWERS_")) {
                let value = value
                    .into_string()
                    .map_err(|_| error("MyPowers environment values must use UTF-8."))?;
                values.insert(key.to_owned(), value);
            }
        }
        let token_file = options
            .get("--token-file")
            .or(values.get("MYPOWERS_API_TOKEN_FILE"));
        let direct = values.get("MYPOWERS_API_TOKEN");
        if !options.contains_key("--token-file") && token_file.is_some() && direct.is_some() {
            return Err(error("Select either API_TOKEN or API_TOKEN_FILE."));
        }
        let token = if let Some(path) = token_file {
            let metadata = fs::metadata(path).map_err(|_| error("Cannot read token file."))?;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(error("Token file must have private permissions (0600)."));
            }
            Some(
                fs::read_to_string(path)
                    .map_err(|_| error("Cannot read token file."))?
                    .trim()
                    .to_owned(),
            )
        } else {
            direct.cloned()
        };
        if let Some(token) = &token {
            let lower = token.to_ascii_lowercase();
            if !(32..=1024).contains(&token.len())
                || !token.is_ascii()
                || token.chars().any(|c| c.is_whitespace() || c.is_control())
                || [
                    "placeholder",
                    "changeme",
                    "change-me",
                    "change_me",
                    "example",
                ]
                .iter()
                .any(|word| lower.contains(word))
            {
                return Err(error("Invalid API token."));
            }
        }
        let origin = options
            .get("--server")
            .or(values.get("MYPOWERS_SERVER_URL"))
            .map(String::as_str)
            .unwrap_or("http://127.0.0.1:8765");
        let server = reqwest::Url::parse(origin).map_err(|_| error("Invalid server URL."))?;
        if !matches!(server.scheme(), "http" | "https")
            || server.host_str().is_none()
            || !server.username().is_empty()
            || server.password().is_some()
            || server.query().is_some()
            || server.fragment().is_some()
            || server.path() != "/"
        {
            return Err(error(
                "Server URL must be an HTTP(S) origin without credentials, path, query or fragment.",
            ));
        }
        let timeout = options
            .get("--timeout")
            .map(|s| s.parse::<f64>())
            .transpose()
            .map_err(|_| error("Invalid timeout."))?
            .unwrap_or(10.0);
        if !timeout.is_finite() || timeout <= 0.0 || timeout > 120.0 {
            return Err(error("Timeout must be between zero and 120 seconds."));
        }
        let timezone = if utc {
            Some(chrono_tz::UTC)
        } else {
            options
                .get("--timezone")
                .or(values.get("MYPOWERS_TIMEZONE"))
                .map(|zone| zone.parse())
                .transpose()
                .map_err(|_| error("Unknown timezone."))?
        };
        let ca = options
            .get("--ca-file")
            .or(values.get("MYPOWERS_CA_FILE"))
            .map(PathBuf::from);
        Ok(Some(Self {
            client_preferences,
            server,
            token,
            ca,
            timeout: Duration::from_secs_f64(timeout),
            no_color,
            no_mouse,
            timezone,
        }))
    }
}
