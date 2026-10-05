use std::{
    collections::HashMap, env, fs, io, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration,
};

pub struct Config {
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
        Self::parse(env::args().skip(1).collect())
    }

    fn parse(args: Vec<String>) -> io::Result<Option<Self>> {
        let mut options = HashMap::new();
        let (mut no_color, mut no_mouse, mut utc) =
            (env::var_os("NO_COLOR").is_some(), false, false);
        let mut arguments = args.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--help" | "-h" => {
                    println!(
                        "MyPowers Ratatui client\n\nUsage: mypowers-tui [options]\n\n  --env-file PATH   Read PATH instead of .env in the current directory\n  --server URL      HTTP(S) daemon origin\n  --token-file PATH Private API token file\n  --ca-file PATH    PEM CA bundle for HTTPS and WSS\n  --timeout SECS    Request timeout (0 < SECS <= 120)\n  --timezone ZONE   IANA timezone for log days/timestamps\n  --utc             UTC log timestamps\n  --no-color        Disable colors\n  --no-mouse        Disable mouse capture\n\na/d/l: AC/DC/lamps; Tab, Enter: focus/activate\nF1: help; F2: dashboard; F3: logs modal; f: log filter\nLogs: r refresh; Left/Right day; +/- page size; End today/live\nr on dashboard: retry; p: pause/resume; b: runtime DEBUG\nEsc: close modal; q: confirm quit; Ctrl-Q: quit immediately\nDouble-click MYPOWERS to copy the current API snapshot (wl-copy)."
                    );
                    return Ok(None);
                }
                "--no-color" => no_color = true,
                "--no-mouse" => no_mouse = true,
                "--utc" => utc = true,
                "--env-file" | "--server" | "--token-file" | "--ca-file" | "--timeout"
                | "--timezone" => {
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
        let mut values = HashMap::new();
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
        values.extend(env::vars().filter(|(key, _)| key.starts_with("MYPOWERS_")));
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
