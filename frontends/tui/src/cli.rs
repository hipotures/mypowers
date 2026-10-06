//! Native one-shot client. Status shares the production dashboard renderer.
use crate::{
    app::App,
    clock::Clock,
    config::Config,
    model::{Command, Status, safe},
    network::{Api, ApiError, Event},
    settings::Settings,
    ui,
};
use chrono::{DateTime, Duration as TimeDelta, Local, NaiveDateTime, TimeZone, Utc};
use crossterm::{
    queue,
    style::{
        Attribute, Color as TerminalColor, ResetColor, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier},
};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, IsTerminal, Write},
    os::unix::process::CommandExt,
    sync::Arc,
    time::Duration,
};
use unicode_width::UnicodeWidthStr;

fn help_text(command: Option<&str>, color: bool) -> String {
    let heading = |text: &str| {
        if color {
            format!("\x1b[1;36m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    };
    let mut text = format!("{}  ·  native client\n\n", heading("MYPOWERS"));
    let mut section = |title: &str, rows: &[(&str, &str)]| {
        text.push_str(&heading(title));
        text.push('\n');
        for (name, description) in rows {
            let label = format!("{name:<25}");
            if color {
                text.push_str(&format!("  \x1b[32m{label}\x1b[0m {description}\n"));
            } else {
                text.push_str(&format!("  {label} {description}\n"));
            }
        }
        text.push('\n');
    };
    match command {
        None => {
            section("Usage", &[("mypowers <command>", "[options]")]);
            section(
                "Monitoring",
                &[
                    ("status", "One-shot dashboard snapshot"),
                    ("capabilities", "Device capabilities"),
                    ("history", "Recorded telemetry"),
                    ("logs", "Application logs and live stream"),
                ],
            );
            section(
                "Control",
                &[
                    ("ac / dc / light", "Set an output: on or off"),
                    ("command <UUID>", "Check a command result"),
                    ("connection", "Bluetooth: pause, resume or retry"),
                    ("debug", "Server debug logging: on or off"),
                ],
            );
            section("Interface", &[("tui", "Interactive terminal dashboard")]);
        }
        Some(name) => {
            let usage = match name {
                "ac" | "dc" | "light" | "debug" => format!("mypowers {name} <on|off> [options]"),
                "connection" => "mypowers connection <pause|resume|retry> [options]".into(),
                "command" => "mypowers command <UUID> [options]".into(),
                _ => format!("mypowers {name} [options]"),
            };
            section("Usage", &[(&usage, "")]);
            match name {
                "status" => section(
                    "Snapshot",
                    &[("--require-live", "Fail unless telemetry is fresh and LIVE")],
                ),
                "history" | "logs" => {
                    section(
                        "Query",
                        &[
                            ("--since TIME", "Start time or relative duration"),
                            ("--until TIME", "End time"),
                            ("--limit N", "Maximum number of records"),
                            ("--cursor CURSOR", "Continue a paginated query"),
                        ],
                    );
                    if name == "logs" {
                        section(
                            "Logs",
                            &[
                                ("--tail N", "Show the most recent records"),
                                ("--level LEVEL", "DEBUG, INFO, WARNING or ERROR"),
                                ("--follow", "Stream new records until Ctrl+C"),
                            ],
                        );
                    }
                }
                "debug" => section(
                    "Debug",
                    &[("--duration TIME", "Debug session duration, e.g. 15m")],
                ),
                _ => {}
            }
        }
    }
    section(
        "Output",
        &[
            ("--json", "Machine-readable JSON"),
            ("--no-color", "Plain text; also respects NO_COLOR"),
            (
                "--timezone ZONE / --utc",
                "Display and query timestamp timezone",
            ),
        ],
    );
    section(
        "Connection",
        &[
            ("--server URL", "HTTP(S) daemon origin"),
            ("--env-file PATH", "Use another dotenv file"),
            ("--client-config PATH", "Local client TOML"),
            ("--token-file PATH", "Private API token file"),
            ("--ca-file PATH", "TLS CA bundle"),
            ("--timeout SECS", "Request timeout"),
        ],
    );
    section(
        "Help",
        &[
            ("-h, --help", "Help for this command"),
            ("--version", "Print version"),
            ("--no-mouse", "Disable mouse capture in TUI"),
        ],
    );
    if command.is_none() {
        text.push_str(
            "Examples\n  mypowers status\n  mypowers logs --follow\n  mypowers history --help\n\n",
        );
    } else if matches!(command, Some("history" | "logs")) {
        text.push_str("TIME: RFC3339, local ISO time with --timezone, or 30m / 1h.\n");
    }
    text.push_str("Global options work before or after the command.\n");
    text
}

fn help_command(raw: &[String]) -> Option<&str> {
    let mut args = raw.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        match arg {
            "--env-file" | "--server" | "--token-file" | "--ca-file" | "--timeout"
            | "--timezone" | "--client-config" => {
                args.next();
            }
            value if value.starts_with('-') => {}
            value => return Some(value),
        }
    }
    None
}

#[derive(Default, Debug)]
struct Arguments {
    command: String,
    positional: Vec<String>,
    config: Vec<String>,
    options: HashMap<String, String>,
    json: bool,
    require_live: bool,
    follow: bool,
}

impl Arguments {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--json" => parsed.json = true,
                "--require-live" => parsed.require_live = true,
                "--follow" => parsed.follow = true,
                "--no-color" | "--utc" | "--no-mouse" => parsed.config.push(arg),
                "--env-file" | "--server" | "--token-file" | "--ca-file" | "--timeout"
                | "--timezone" | "--client-config" => {
                    let value = args.next().ok_or("Missing option value. Use --help.")?;
                    parsed.config.extend([arg, value]);
                }
                "--since" | "--until" | "--limit" | "--cursor" | "--tail" | "--level"
                | "--duration" => {
                    let value = args.next().ok_or("Missing option value. Use --help.")?;
                    if value.starts_with("--") {
                        return Err("Missing option value.".into());
                    }
                    parsed.options.insert(arg, value);
                }
                value if value.starts_with('-') => return Err("Unknown option. Use --help.".into()),
                _ if parsed.command.is_empty() => parsed.command = arg,
                _ => parsed.positional.push(arg),
            }
        }
        let positions: &[&str] = match parsed.command.as_str() {
            "status" | "capabilities" | "history" | "logs" | "tui" => &[],
            "ac" | "dc" | "light" | "debug" => &["on", "off"],
            "connection" => &["pause", "resume", "retry"],
            "command" => &["UUID"],
            _ => return Err("Select a command. Use --help.".into()),
        };
        if positions.is_empty() {
            if !parsed.positional.is_empty() {
                return Err("Unexpected positional argument.".into());
            }
        } else if parsed.positional.len() != 1
            || (positions != ["UUID"] && !positions.contains(&parsed.positional[0].as_str()))
        {
            return Err("Invalid command argument. Use --help.".into());
        }
        if parsed.command == "command" && uuid::Uuid::parse_str(&parsed.positional[0]).is_err() {
            return Err("Invalid command UUID.".into());
        }
        if parsed.require_live && parsed.command != "status"
            || parsed.follow && parsed.command != "logs"
        {
            return Err("Option is not valid for this command.".into());
        }
        for option in parsed.options.keys() {
            let valid = match option.as_str() {
                "--duration" => parsed.command == "debug",
                "--tail" | "--level" => parsed.command == "logs",
                _ => matches!(parsed.command.as_str(), "history" | "logs"),
            };
            if !valid {
                return Err("Option is not valid for this command.".into());
            }
        }
        if let Some(level) = parsed.options.get("--level")
            && !crate::logs::LEVELS.contains(&level.as_str())
        {
            return Err("Unknown log level.".into());
        }
        for (name, max) in [
            (
                "--limit",
                if parsed.command == "history" {
                    10000
                } else {
                    1000
                },
            ),
            ("--tail", 1000),
        ] {
            if let Some(value) = parsed.options.get(name) {
                let number = value.parse::<u32>().map_err(|_| "Invalid query count.")?;
                if number == 0 || number > max {
                    return Err("Query count is outside its allowed range.".into());
                }
            }
        }
        if let Some(value) = parsed.options.get("--duration") {
            duration(value)?;
        }
        Ok(parsed)
    }
}

fn duration(value: &str) -> Result<f64, String> {
    let (number, multiplier) = match value.chars().last() {
        Some('s') => (&value[..value.len() - 1], 1.0),
        Some('m') => (&value[..value.len() - 1], 60.0),
        Some('h') => (&value[..value.len() - 1], 3600.0),
        Some('d') => (&value[..value.len() - 1], 86400.0),
        _ => (value, 1.0),
    };
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err("Use a duration such as 30s, 15m or 1h.".into());
    }
    let seconds = number.parse::<f64>().map_err(|_| "Invalid duration.")? * multiplier;
    if !seconds.is_finite() || seconds <= 0.0 || seconds > 86400.0 {
        return Err("Duration must be greater than zero and at most one day.".into());
    }
    Ok(seconds)
}

fn query_time(value: &str, now: &str, config: &Config) -> Result<String, String> {
    let time = if value.ends_with(['s', 'm', 'h', 'd']) && !value.contains('T') {
        DateTime::parse_from_rfc3339(now)
            .map_err(|_| "Invalid daemon timestamp.")?
            .with_timezone(&Utc)
            - TimeDelta::milliseconds((duration(value)? * 1000.0) as i64)
    } else if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        time.with_timezone(&Utc)
    } else {
        let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
            .map_err(|_| "Invalid timestamp.")?;
        config
            .timezone
            .ok_or("Local timestamps require --timezone or --utc.")?
            .from_local_datetime(&naive)
            .single()
            .ok_or("Ambiguous or nonexistent local timestamp; supply a UTC offset.")?
            .with_timezone(&Utc)
    };
    Ok(time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn output_error(error: io::Error) -> ApiError {
    ApiError {
        code: if error.kind() == io::ErrorKind::BrokenPipe {
            "broken_pipe"
        } else {
            "output_error"
        }
        .into(),
        message: "Cannot write client output.".into(),
        status: 0,
    }
}

fn invalid(message: &str) -> ApiError {
    message.into()
}

fn command_exit(value: &Value) -> i32 {
    match value["status"].as_str() {
        Some("unconfirmed") => 6,
        Some("rejected") => 5,
        Some("failed") => 1,
        _ => 0,
    }
}

fn error_exit(error: &ApiError) -> i32 {
    if error.code == "admission_unknown" {
        6
    } else {
        match error.status {
            0 => 3,
            401 | 403 => 4,
            409 | 503 => 5,
            _ => 1,
        }
    }
}

async fn status(api: &Api) -> Result<(Value, Status), ApiError> {
    let raw = api
        .request_detailed(Method::GET, "/status", None, None)
        .await?;
    let status: Status =
        serde_json::from_value(raw.clone()).map_err(|_| invalid("Invalid status response."))?;
    if !status.valid() {
        return Err(invalid("Invalid status response."));
    }
    Ok((raw, status))
}

fn checked_command(
    raw: &Value,
    id: Option<&str>,
    intention: Option<(&str, bool)>,
) -> Result<Command, ApiError> {
    let command: Command =
        serde_json::from_value(raw.clone()).map_err(|_| invalid("Invalid command response."))?;
    if !command.valid()
        || id.is_some_and(|id| {
            uuid::Uuid::parse_str(&command.command_id).ok() != uuid::Uuid::parse_str(id).ok()
        })
        || intention.is_some_and(|(output, enabled)| {
            command.output != output || command.requested_enabled != enabled
        })
    {
        return Err(invalid(
            "Invalid command receipt or changed output intention.",
        ));
    }
    Ok(command)
}

async fn output(api: &Api, name: &str, enabled: bool) -> Result<Value, ApiError> {
    let (_, current) = status(api).await?;
    let key = uuid::Uuid::new_v4().to_string();
    let body = json!({"enabled":enabled,"server_instance_id":current.server_instance_id,"expected_outputs_revision":current.controls.outputs_revision});
    let uncertain = || ApiError {
        code: "admission_unknown".into(),
        message: format!("Admission uncertain; idempotency key {key}. Do not replay."),
        status: 0,
    };
    let mut raw = api
        .request_detailed(
            Method::PUT,
            &format!("/outputs/{name}"),
            Some(body),
            Some(&key),
        )
        .await
        .map_err(|e| if e.status == 0 { uncertain() } else { e })?;
    let first = checked_command(&raw, None, Some((name, enabled))).map_err(|_| uncertain())?;
    let id = first.command_id;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    while !checked_command(&raw, Some(&id), Some((name, enabled)))?.terminal() {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let result = tokio::time::timeout(
            remaining,
            api.request_detailed(Method::GET, &format!("/commands/{id}"), None, None),
        )
        .await;
        match result {
            Ok(Ok(next)) if checked_command(&next, Some(&id), Some((name, enabled))).is_ok() => {
                raw = next
            }
            _ => {
                raw["status"] = json!("unconfirmed");
                raw["reason_code"] = json!(if remaining.is_zero() {
                    "client_wait_expired"
                } else {
                    "client_connection_lost"
                });
                break;
            }
        }
    }
    Ok(raw)
}

pub fn snapshot_buffer(mut app: App, width: u16, height: u16) -> Result<Buffer, String> {
    app.snapshot = true;
    app.selected = None;
    app.hovered = None;
    app.feedback = None;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).map_err(|e| e.to_string())?;
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .map_err(|e| e.to_string())?;
    Ok(terminal.backend().buffer().clone())
}

fn terminal_color(color: Color) -> TerminalColor {
    match color {
        Color::Reset => TerminalColor::Reset,
        Color::Black => TerminalColor::Black,
        Color::Red => TerminalColor::DarkRed,
        Color::Green => TerminalColor::DarkGreen,
        Color::Yellow => TerminalColor::DarkYellow,
        Color::Blue => TerminalColor::DarkBlue,
        Color::Magenta => TerminalColor::DarkMagenta,
        Color::Cyan => TerminalColor::DarkCyan,
        Color::Gray => TerminalColor::Grey,
        Color::DarkGray => TerminalColor::DarkGrey,
        Color::LightRed => TerminalColor::Red,
        Color::LightGreen => TerminalColor::Green,
        Color::LightYellow => TerminalColor::Yellow,
        Color::LightBlue => TerminalColor::Blue,
        Color::LightMagenta => TerminalColor::Magenta,
        Color::LightCyan => TerminalColor::Cyan,
        Color::White => TerminalColor::White,
        Color::Indexed(n) => TerminalColor::AnsiValue(n),
        Color::Rgb(r, g, b) => TerminalColor::Rgb { r, g, b },
    }
}

fn write_buffer(out: &mut impl Write, buffer: &Buffer, color: bool) -> io::Result<()> {
    let mut style = None;
    for row in buffer.content.chunks(usize::from(buffer.area.width)) {
        let mut skip = 0;
        for cell in row {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if color && style != Some((cell.fg, cell.bg, cell.modifier)) {
                queue!(
                    out,
                    SetAttribute(Attribute::Reset),
                    SetForegroundColor(terminal_color(cell.fg)),
                    SetBackgroundColor(terminal_color(cell.bg))
                )?;
                for (modifier, attribute) in [
                    (Modifier::BOLD, Attribute::Bold),
                    (Modifier::DIM, Attribute::Dim),
                    (Modifier::ITALIC, Attribute::Italic),
                    (Modifier::UNDERLINED, Attribute::Underlined),
                    (Modifier::REVERSED, Attribute::Reverse),
                ] {
                    if cell.modifier.contains(modifier) {
                        queue!(out, SetAttribute(attribute))?;
                    }
                }
                style = Some((cell.fg, cell.bg, cell.modifier));
            }
            write!(out, "{}", cell.symbol())?;
            skip = cell.symbol().width().saturating_sub(1);
        }
        if color {
            queue!(out, ResetColor, SetAttribute(Attribute::Reset))?;
            style = None;
        }
        writeln!(out)?;
    }
    out.flush()
}

async fn snapshot(
    api: &Api,
    status: Status,
    config: &Config,
    out: &mut impl Write,
) -> Result<(), ApiError> {
    let settings: Settings = serde_json::from_value(
        api.request_detailed(Method::GET, "/settings", None, None)
            .await?,
    )
    .map_err(|_| invalid("Invalid settings response."))?;
    if !settings.valid() {
        return Err(invalid("Invalid settings response."));
    }
    let now = DateTime::parse_from_rfc3339(&status.server_time)
        .map_err(|_| invalid("Invalid server timestamp."))?
        .with_timezone(&Utc);
    let color = !config.no_color && io::stdout().is_terminal();
    let timezone = config.timezone.or_else(|| settings.timezone.parse().ok());
    let mut app = App::with_clock(
        !color,
        timezone,
        Clock::Fixed {
            now,
            telemetry_elapsed: Duration::ZERO,
            animation_elapsed: Duration::ZERO,
            feedback_elapsed: Duration::ZERO,
        },
    );
    app.client_preferences = config.client_preferences.clone();
    app.snapshot = true;
    app.selected = None;
    app.feedback = None;
    app.connected = true;
    app.update(Event::Settings(settings));
    app.status = Some(status);
    let width = crossterm::terminal::size()
        .map(|(w, _)| w.min(98))
        .unwrap_or(98);
    let height = 31;
    // First render establishes the exact plot width used by the production chart.
    let mut terminal = Terminal::new(TestBackend::new(width, height))
        .map_err(|_| invalid("Cannot render snapshot."))?;
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .map_err(|_| invalid("Cannot render snapshot."))?;
    if let Some(request) = app.history_request() {
        app.graph.points = api.history(&request).await.map_err(|e| invalid(&e))?;
    }
    let buffer = snapshot_buffer(app, width, height).map_err(|e| invalid(&e))?;
    write_buffer(out, &buffer, color).map_err(|e| ApiError {
        code: if e.kind() == io::ErrorKind::BrokenPipe {
            "broken_pipe"
        } else {
            "output_error"
        }
        .into(),
        message: "Cannot write client output.".into(),
        status: 0,
    })
}

fn clean(value: &Value, token: Option<&str>) -> Value {
    match value {
        Value::String(text) => Value::String(safe(text).replace(
            token.filter(|s| !s.is_empty()).unwrap_or("\0"),
            "[redacted]",
        )),
        Value::Array(values) => Value::Array(values.iter().map(|v| clean(v, token)).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(k, v)| (k.clone(), clean(v, token)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn display_time(value: &str, config: &Config) -> String {
    if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        if let Some(zone) = config.timezone {
            time.with_timezone(&zone)
                .format("%Y-%m-%d %H:%M:%S %Z")
                .to_string()
        } else {
            time.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S %Z")
                .to_string()
        }
    } else {
        safe(value).into_owned()
    }
}

fn emit(out: &mut impl Write, value: &Value, args: &Arguments, config: &Config) -> io::Result<()> {
    if args.json {
        writeln!(out, "{}", serde_json::to_string(value)?)?;
    } else {
        let value = clean(value, config.token.as_deref());
        if value.get("command_id").is_some() {
            writeln!(
                out,
                "{} {} | Command {}",
                value["output"].as_str().unwrap_or("").to_uppercase(),
                value["status"].as_str().unwrap_or("unknown"),
                value["command_id"].as_str().unwrap_or("")
            )?;
            if let Some(reason) = value["reason_code"].as_str() {
                writeln!(out, "{reason}")?;
            }
            if value["status"] == "unconfirmed" {
                writeln!(
                    out,
                    "The output may have changed. Query command/status; do not replay."
                )?;
            }
        } else if let Some(items) = value["items"].as_array() {
            if args.command == "logs" {
                for record in items {
                    writeln!(
                        out,
                        "{}  {}  {}  {}",
                        display_time(record["timestamp"].as_str().unwrap_or(""), config),
                        record["level"].as_str().unwrap_or(""),
                        record["event"].as_str().unwrap_or(""),
                        record["message"].as_str().unwrap_or("")
                    )?;
                }
            } else {
                writeln!(out, "Time  Battery  Input W  Output W  Segment")?;
                for record in items {
                    let time = record["received_at_ms"]
                        .as_i64()
                        .and_then(DateTime::from_timestamp_millis)
                        .map(|t| display_time(&t.to_rfc3339(), config))
                        .unwrap_or_default();
                    writeln!(
                        out,
                        "{}  {}%  {}  {}  {}",
                        time,
                        record["battery_percent"],
                        record["input_power_w"],
                        record["output_power_w"],
                        record["segment_id"].as_str().unwrap_or("")
                    )?;
                }
            }
            if let Some(cursor) = value["next_cursor"].as_str() {
                writeln!(out, "Next cursor: {cursor}")?;
            }
        } else {
            writeln!(out, "{}", serde_json::to_string_pretty(&value)?)?;
        }
    }
    out.flush()
}

fn query_path(path: &str, params: &[(String, String)]) -> String {
    let mut url = reqwest::Url::parse("http://localhost").unwrap();
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(k, v)| (k, v)));
    format!("{path}?{}", url.query().unwrap_or(""))
}

async fn run(args: &Arguments, config: &Config, out: &mut impl Write) -> Result<i32, ApiError> {
    let api = Arc::new(Api::new(config).map_err(|message| ApiError {
        code: "invalid_usage".into(),
        message,
        status: 400,
    })?);
    let name = args.command.as_str();
    let data = match name {
        "status" => {
            let (raw, state) = status(&api).await?;
            let exit = if args.require_live && state.telemetry.state != "live" {
                5
            } else {
                0
            };
            if args.json {
                emit(out, &raw, args, config).map_err(output_error)?;
            } else {
                snapshot(&api, state, config, out).await?;
            }
            return Ok(exit);
        }
        "ac" | "dc" | "light" => output(&api, name, args.positional[0] == "on").await?,
        "command" => {
            let id = &args.positional[0];
            let raw = api
                .request_detailed(Method::GET, &format!("/commands/{id}"), None, None)
                .await?;
            checked_command(&raw, Some(id), None)?;
            raw
        }
        "capabilities" => {
            api.request_detailed(Method::GET, "/capabilities", None, None)
                .await?
        }
        "history" | "logs" => {
            let (_, state) = status(&api).await?;
            let mut params = Vec::new();
            for flag in ["--since", "--until"] {
                if let Some(value) = args.options.get(flag) {
                    params.push((
                        flag[2..].to_owned(),
                        query_time(value, &state.server_time, config).map_err(|e| ApiError {
                            code: "invalid_usage".into(),
                            message: e,
                            status: 400,
                        })?,
                    ));
                }
            }
            for flag in ["--limit", "--cursor", "--tail"] {
                if let Some(value) = args.options.get(flag) {
                    params.push((flag[2..].to_owned(), value.clone()));
                }
            }
            if name == "logs" {
                params.push((
                    "min_level".into(),
                    args.options
                        .get("--level")
                        .cloned()
                        .unwrap_or("DEBUG".into()),
                ));
                if !["--since", "--until", "--cursor", "--tail"]
                    .iter()
                    .any(|flag| args.options.contains_key(*flag))
                {
                    params.push(("tail".into(), "10".into()));
                }
            }
            let raw = api
                .request_detailed(
                    Method::GET,
                    &query_path(&format!("/{name}"), &params),
                    None,
                    None,
                )
                .await?;
            emit(out, &raw, args, config).map_err(output_error)?;
            if name == "logs" && args.follow {
                let mut params = vec![(
                    "min_level".into(),
                    args.options
                        .get("--level")
                        .cloned()
                        .unwrap_or("DEBUG".into()),
                )];
                if let Some(cursor) = raw["next_cursor"].as_str() {
                    params.push(("cursor".into(), cursor.into()));
                }
                let query = query_path("", &params)[1..].to_owned();
                let (sender, mut receiver) = tokio::sync::mpsc::channel(128);
                let api = api.clone();
                let stream_task = tokio::spawn(async move {
                    if let Err(error) = api.follow_logs(&query, &sender).await {
                        let _ = sender.send(Event::Disconnected(error)).await;
                    }
                });
                loop {
                    tokio::select! {
                        event = receiver.recv() => match event {
                            Some(Event::Log(record)) => emit(out,&if args.json {record} else {json!({"items":[record]})},args,config).map_err(output_error)?,
                            Some(Event::Notice(message)) => eprintln!("{}",safe(&message)),
                            Some(Event::Disconnected(message)) => return Err(invalid(&message)),
                            None => break,
                            _ => {},
                        },
                        _ = tokio::signal::ctrl_c() => { stream_task.abort(); return Ok(130); },
                    }
                }
            }
            return Ok(0);
        }
        "debug" => {
            if args.positional[0] == "off" {
                api.request_detailed(Method::DELETE, "/runtime/log-level", None, None)
                    .await?
            } else {
                let seconds = args.options.get("--duration").map(|v| duration(v).unwrap());
                api.request_detailed(
                    Method::PUT,
                    "/runtime/log-level",
                    Some(json!({"level":"DEBUG","duration_seconds":seconds})),
                    None,
                )
                .await?
            }
        }
        "connection" => {
            if args.positional[0] == "retry" {
                api.request_detailed(Method::POST, "/connection/retry", Some(json!({})), None)
                    .await?
            } else {
                api.request_detailed(Method::PUT,"/connection",Some(json!({"desired":if args.positional[0]=="pause" {"paused"} else {"running"}})),None).await?
            }
        }
        _ => return Err(invalid("Unknown command.")),
    };
    emit(out, &data, args, config).map_err(output_error)?;
    Ok(command_exit(&data))
}

pub fn main() -> i32 {
    let raw: Vec<_> = std::env::args().skip(1).collect();
    if raw
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        let color = io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && !raw.iter().any(|arg| arg == "--no-color");
        print!("{}", help_text(help_command(&raw), color));
        return 0;
    }
    if raw == ["--version"] {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let args = match Arguments::parse(raw) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    if args.command == "tui" {
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|p| p.join("mypowers-tui")))
            .filter(|p| p.is_file());
        let error = std::process::Command::new(sibling.unwrap_or_else(|| "mypowers-tui".into()))
            .args(&args.config)
            .exec();
        eprintln!(
            "Cannot start native TUI ({:?}). Install: cargo install --locked --path frontends/tui",
            error.kind()
        );
        return 2;
    }
    let config = match Config::parse(args.config.clone()) {
        Ok(Some(config)) => config,
        Ok(None) => return 0,
        Err(error) => {
            eprintln!("Client configuration error: {error}");
            return 2;
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("Cannot start native client runtime.");
            return 1;
        }
    };
    let mut out = io::stdout().lock();
    match runtime.block_on(run(&args, &config, &mut out)) {
        Ok(code) => code,
        Err(error) => {
            if error.code == "broken_pipe" {
                return 0;
            }
            let message = clean(&json!(error.message), config.token.as_deref());
            if args.json {
                let _ = writeln!(
                    out,
                    "{}",
                    clean(
                        &json!({"schema_version":1,"error":{"code":error.code,"message":message,"retryable":false,"request_id":""}}),
                        config.token.as_deref()
                    )
                );
            } else {
                eprintln!("{}", message.as_str().unwrap_or("Client error"));
            }
            if error.code == "invalid_usage" {
                2
            } else {
                error_exit(&error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_is_grouped_and_command_options_are_local() {
        let root = help_text(None, false);
        assert!(root.contains("Monitoring\n") && root.contains("Control\n"));
        assert!(!root.contains("--since") && !root.contains('\x1b'));
        let logs = help_text(Some("logs"), false);
        assert!(logs.contains("mypowers logs [options]") && logs.contains("--follow"));
        assert!(logs.contains("--cursor") && !logs.contains("--require-live"));
        assert!(help_text(Some("status"), false).contains("--require-live"));
        assert!(help_text(None, true).contains("\x1b[1;36m"));
        let raw = ["--token-file", "logs", "status", "--help"].map(str::to_owned);
        assert_eq!(help_command(&raw), Some("status"));
    }

    #[test]
    fn command_arguments_before_and_after_subcommand_and_ranges() {
        for args in [
            "--json status",
            "status --json --require-live",
            "--env-file selected.env logs --tail 10 --follow",
            "ac on",
            "dc off",
            "light on",
            "history --since 1h --limit 100",
            "debug on --duration 15m",
            "connection retry",
            "tui --no-mouse",
        ] {
            assert!(
                Arguments::parse(args.split_whitespace().map(str::to_owned).collect()).is_ok(),
                "{args}"
            );
        }
        for args in [
            "ac toggle",
            "status --follow",
            "logs --tail 0",
            "history --limit 10001",
            "status --tail 2",
            "debug on --duration 2d",
            "command invalid",
            "logs --level UNKNOWN",
        ] {
            assert!(
                Arguments::parse(args.split_whitespace().map(str::to_owned).collect()).is_err(),
                "{args}"
            );
        }
    }

    #[test]
    fn durations_and_timezone_queries_keep_old_cli_semantics() {
        let config = Config {
            client_preferences: crate::client_ui::ClientPreferences::default(),
            server: reqwest::Url::parse("http://localhost").unwrap(),
            token: None,
            ca: None,
            timeout: Duration::from_secs(1),
            no_color: true,
            no_mouse: true,
            timezone: Some(chrono_tz::Europe::Warsaw),
        };
        assert_eq!(duration("15m").unwrap(), 900.0);
        assert_eq!(
            query_time("1h", "2026-10-04T12:00:00Z", &config).unwrap(),
            "2026-10-04T11:00:00.000Z"
        );
        assert_eq!(
            query_time("2026-10-04T12:00:00", "", &config).unwrap(),
            "2026-10-04T10:00:00.000Z"
        );
        assert!(query_time("2026-10-25T02:30:00", "", &config).is_err());
        for bad in ["0s", "2d", "wrong", "NaN", "-1m"] {
            assert!(duration(bad).is_err());
        }
        assert_eq!(command_exit(&json!({"status":"unconfirmed"})), 6);
        assert_eq!(command_exit(&json!({"status":"rejected"})), 5);
        assert_eq!(command_exit(&json!({"status":"failed"})), 1);
    }

    #[test]
    fn snapshot_removes_focus_and_hotkeys_but_preserves_real_output_states() {
        let mut status = crate::tests::status();
        status.telemetry.sample.as_mut().unwrap().dc_enabled = true;
        let mut app = App::new(false, None);
        app.connected = true;
        app.status = Some(status);
        app.selected = Some(0);
        app.hovered = Some(1);
        let buffer = snapshot_buffer(app, 98, 31).unwrap();
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("MYPOWERS") && text.contains("OFF") && text.contains("ON"));
        assert!(
            !text.contains("q quit")
                && !text.contains("F3 logs")
                && !text.contains("Connecting to daemon")
        );
        let states: Vec<_> = buffer
            .content
            .iter()
            .filter(|cell| cell.symbol() == "O")
            .collect();
        assert!(
            states
                .iter()
                .any(|cell| cell.fg == Color::Rgb(118, 203, 137))
        );
        assert!(states.iter().all(|cell| cell.bg != Color::Rgb(34, 46, 54)));
        let mut plain = Vec::new();
        write_buffer(&mut plain, &buffer, false).unwrap();
        assert!(!plain.contains(&0x1b));
        let mut colored = Vec::new();
        write_buffer(&mut colored, &buffer, true).unwrap();
        let ansi = String::from_utf8(colored).unwrap();
        assert!(ansi.contains("\x1b["));
        assert!(!ansi.contains("?1049") && !ansi.contains("?25") && !ansi.contains("?1000"));
    }

    #[test]
    fn changed_polling_receipt_cannot_confirm_a_different_output_intention() {
        let raw = json!({"schema_version":1,"command_id":"88767477-2a2a-481f-843b-30d56a5e3f10","status":"confirmed","output":"ac","requested_enabled":true,"reason_code":null});
        assert!(checked_command(&raw, None, Some(("ac", true))).is_ok());
        assert!(checked_command(&raw, Some("88767477-2A2A-481F-843B-30D56A5E3F10"), None).is_ok());
        assert!(checked_command(&raw, None, Some(("dc", true))).is_err());
        assert!(checked_command(&raw, Some("different-id"), None).is_err());
    }
}
