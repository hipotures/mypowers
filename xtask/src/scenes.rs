//! Fixture or read-only captured state; layout lives in the production TUI crate.
use chrono::{DateTime, Duration as TimeDelta, Utc};
use mypowers_tui::{
    app::{App, View},
    clock::Clock,
    feedback::{Feedback, Severity},
    history::{Point, Resolution, Visualization},
    model::Status,
    settings::{Field, Settings, SettingsTab},
    ui,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use serde_json::json;
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum Scene {
    Themed(ThemeView, &'static str),
    Snapshot,
    Live,
    LiveMinute,
    LiveHour,
    Chart,
    ChartMinute,
    ChartHour,
    ChartNearby,
    ChartGaps,
    ChartIdle,
    ChartLowLoad,
    ChartOffline,
    LowLoad,
    Idle,
    Reconnecting,
    DeviceOffline,
    DaemonOffline,
    CommandPending,
    Logs,
    LogsOldest,
    Settings,
    SettingsCharts,
    SettingsIntervalPicker,
    SettingsVisualizationPicker,
    SettingsAlerts,
    SettingsNotify,
    SettingsDebug,
    SettingsTheme(&'static str),
    SettingsHelp,
    Help,
    LogsHelp,
    Quit,
    Picker(Field),
    SettingsHelpTab(SettingsTab),
    NotifyState(NotifyState),
    LogsLevel(usize),
    ThirtySeconds(bool),
    DebugState(bool),
}

#[derive(Clone, Copy)]
pub enum ThemeView {
    Dashboard,
    Chart,
    Logs,
    Help,
    Quit,
    Small,
}

#[derive(Clone, Copy)]
pub enum NotifyState {
    Configured,
    Sending,
    Sent,
    Failed,
}

/// Additional views for publication; the gallery exporter supplies one shared canvas size.
pub const GALLERY: &[(&str, Scene, u16, u16)] = &[
    (
        "dashboard-live-30s.svg",
        Scene::ThirtySeconds(false),
        120,
        30,
    ),
    (
        "dashboard-chart-30s.svg",
        Scene::ThirtySeconds(true),
        120,
        30,
    ),
    ("logs-info.svg", Scene::LogsLevel(1), 120, 30),
    ("logs-warning.svg", Scene::LogsLevel(2), 120, 30),
    ("logs-error.svg", Scene::LogsLevel(3), 120, 30),
    (
        "settings-debug-enabled.svg",
        Scene::DebugState(false),
        120,
        30,
    ),
    (
        "settings-debug-paused.svg",
        Scene::DebugState(true),
        120,
        30,
    ),
    (
        "settings-timezone-picker.svg",
        Scene::Picker(Field::Timezone),
        120,
        30,
    ),
    (
        "settings-page-size-picker.svg",
        Scene::Picker(Field::PageSize),
        120,
        30,
    ),
    (
        "settings-scale-picker.svg",
        Scene::Picker(Field::Scale),
        120,
        30,
    ),
    (
        "settings-alert-enabled-picker.svg",
        Scene::Picker(Field::AlertEnabled),
        120,
        30,
    ),
    (
        "settings-alert-threshold-picker.svg",
        Scene::Picker(Field::AlertThreshold),
        120,
        30,
    ),
    (
        "settings-alert-hysteresis-picker.svg",
        Scene::Picker(Field::AlertHysteresis),
        120,
        30,
    ),
    (
        "settings-alert-cooldown-picker.svg",
        Scene::Picker(Field::AlertCooldown),
        120,
        30,
    ),
    (
        "help-preferences.svg",
        Scene::SettingsHelpTab(SettingsTab::Preferences),
        120,
        30,
    ),
    (
        "help-charts.svg",
        Scene::SettingsHelpTab(SettingsTab::Charts),
        120,
        30,
    ),
    (
        "help-alerts.svg",
        Scene::SettingsHelpTab(SettingsTab::Alerts),
        120,
        30,
    ),
    (
        "help-notify.svg",
        Scene::SettingsHelpTab(SettingsTab::Notify),
        120,
        30,
    ),
    (
        "settings-notify-configured.svg",
        Scene::NotifyState(NotifyState::Configured),
        120,
        30,
    ),
    (
        "settings-notify-sending.svg",
        Scene::NotifyState(NotifyState::Sending),
        120,
        30,
    ),
    (
        "settings-notify-sent.svg",
        Scene::NotifyState(NotifyState::Sent),
        120,
        30,
    ),
    (
        "settings-notify-failed.svg",
        Scene::NotifyState(NotifyState::Failed),
        120,
        30,
    ),
];

pub const SCENES: &[(&str, Scene, u16, u16)] = &[
    (
        "theme-mypowers-dashboard.svg",
        Scene::Themed(ThemeView::Dashboard, "MyPowers"),
        120,
        30,
    ),
    (
        "theme-mypowers-chart.svg",
        Scene::Themed(ThemeView::Chart, "MyPowers"),
        120,
        30,
    ),
    (
        "theme-mypowers-logs.svg",
        Scene::Themed(ThemeView::Logs, "MyPowers"),
        120,
        30,
    ),
    (
        "theme-mypowers-help.svg",
        Scene::Themed(ThemeView::Help, "MyPowers"),
        120,
        30,
    ),
    (
        "theme-mypowers-quit.svg",
        Scene::Themed(ThemeView::Quit, "MyPowers"),
        120,
        30,
    ),
    (
        "theme-mypowers-small.svg",
        Scene::Themed(ThemeView::Small, "MyPowers"),
        50,
        14,
    ),
    (
        "theme-catppuccin-dashboard.svg",
        Scene::Themed(ThemeView::Dashboard, "Catppuccin"),
        120,
        30,
    ),
    (
        "theme-catppuccin-chart.svg",
        Scene::Themed(ThemeView::Chart, "Catppuccin"),
        120,
        30,
    ),
    (
        "theme-catppuccin-logs.svg",
        Scene::Themed(ThemeView::Logs, "Catppuccin"),
        120,
        30,
    ),
    (
        "theme-catppuccin-help.svg",
        Scene::Themed(ThemeView::Help, "Catppuccin"),
        120,
        30,
    ),
    (
        "theme-catppuccin-quit.svg",
        Scene::Themed(ThemeView::Quit, "Catppuccin"),
        120,
        30,
    ),
    (
        "theme-catppuccin-small.svg",
        Scene::Themed(ThemeView::Small, "Catppuccin"),
        50,
        14,
    ),
    (
        "theme-nord-dashboard.svg",
        Scene::Themed(ThemeView::Dashboard, "Nord"),
        120,
        30,
    ),
    (
        "theme-nord-chart.svg",
        Scene::Themed(ThemeView::Chart, "Nord"),
        120,
        30,
    ),
    (
        "theme-nord-logs.svg",
        Scene::Themed(ThemeView::Logs, "Nord"),
        120,
        30,
    ),
    (
        "theme-nord-help.svg",
        Scene::Themed(ThemeView::Help, "Nord"),
        120,
        30,
    ),
    (
        "theme-nord-quit.svg",
        Scene::Themed(ThemeView::Quit, "Nord"),
        120,
        30,
    ),
    (
        "theme-nord-small.svg",
        Scene::Themed(ThemeView::Small, "Nord"),
        50,
        14,
    ),
    (
        "theme-gruvbox-dashboard.svg",
        Scene::Themed(ThemeView::Dashboard, "Gruvbox"),
        120,
        30,
    ),
    (
        "theme-gruvbox-chart.svg",
        Scene::Themed(ThemeView::Chart, "Gruvbox"),
        120,
        30,
    ),
    (
        "theme-gruvbox-logs.svg",
        Scene::Themed(ThemeView::Logs, "Gruvbox"),
        120,
        30,
    ),
    (
        "theme-gruvbox-help.svg",
        Scene::Themed(ThemeView::Help, "Gruvbox"),
        120,
        30,
    ),
    (
        "theme-gruvbox-quit.svg",
        Scene::Themed(ThemeView::Quit, "Gruvbox"),
        120,
        30,
    ),
    (
        "theme-gruvbox-small.svg",
        Scene::Themed(ThemeView::Small, "Gruvbox"),
        50,
        14,
    ),
    (
        "theme-tokyo-night-dashboard.svg",
        Scene::Themed(ThemeView::Dashboard, "Tokyo Night"),
        120,
        30,
    ),
    (
        "theme-tokyo-night-chart.svg",
        Scene::Themed(ThemeView::Chart, "Tokyo Night"),
        120,
        30,
    ),
    (
        "theme-tokyo-night-logs.svg",
        Scene::Themed(ThemeView::Logs, "Tokyo Night"),
        120,
        30,
    ),
    (
        "theme-tokyo-night-help.svg",
        Scene::Themed(ThemeView::Help, "Tokyo Night"),
        120,
        30,
    ),
    (
        "theme-tokyo-night-quit.svg",
        Scene::Themed(ThemeView::Quit, "Tokyo Night"),
        120,
        30,
    ),
    (
        "theme-tokyo-night-small.svg",
        Scene::Themed(ThemeView::Small, "Tokyo Night"),
        50,
        14,
    ),
    (
        "theme-solarized-dashboard.svg",
        Scene::Themed(ThemeView::Dashboard, "Solarized"),
        120,
        30,
    ),
    (
        "theme-solarized-chart.svg",
        Scene::Themed(ThemeView::Chart, "Solarized"),
        120,
        30,
    ),
    (
        "theme-solarized-logs.svg",
        Scene::Themed(ThemeView::Logs, "Solarized"),
        120,
        30,
    ),
    (
        "theme-solarized-help.svg",
        Scene::Themed(ThemeView::Help, "Solarized"),
        120,
        30,
    ),
    (
        "theme-solarized-quit.svg",
        Scene::Themed(ThemeView::Quit, "Solarized"),
        120,
        30,
    ),
    (
        "theme-solarized-small.svg",
        Scene::Themed(ThemeView::Small, "Solarized"),
        50,
        14,
    ),
    (
        "theme-terminal-dashboard.svg",
        Scene::Themed(ThemeView::Dashboard, "Terminal"),
        120,
        30,
    ),
    (
        "theme-terminal-chart.svg",
        Scene::Themed(ThemeView::Chart, "Terminal"),
        120,
        30,
    ),
    (
        "theme-terminal-logs.svg",
        Scene::Themed(ThemeView::Logs, "Terminal"),
        120,
        30,
    ),
    (
        "theme-terminal-help.svg",
        Scene::Themed(ThemeView::Help, "Terminal"),
        120,
        30,
    ),
    (
        "theme-terminal-quit.svg",
        Scene::Themed(ThemeView::Quit, "Terminal"),
        120,
        30,
    ),
    (
        "theme-terminal-small.svg",
        Scene::Themed(ThemeView::Small, "Terminal"),
        50,
        14,
    ),
    ("cli-status.svg", Scene::Snapshot, 120, 30),
    ("dashboard-live.svg", Scene::Live, 120, 30),
    ("dashboard-live-60s.svg", Scene::LiveMinute, 120, 30),
    ("dashboard-live-1h.svg", Scene::LiveHour, 120, 30),
    ("dashboard-live-80x24.svg", Scene::Live, 80, 24),
    ("dashboard-live-60x19.svg", Scene::Live, 60, 19),
    ("dashboard-chart.svg", Scene::Chart, 120, 30),
    ("dashboard-chart-nearby.svg", Scene::ChartNearby, 120, 30),
    ("dashboard-chart-60s.svg", Scene::ChartMinute, 120, 30),
    ("dashboard-chart-1h.svg", Scene::ChartHour, 120, 30),
    ("dashboard-chart-80x24.svg", Scene::Chart, 80, 24),
    ("dashboard-chart-60x19.svg", Scene::Chart, 60, 19),
    ("dashboard-chart-gaps.svg", Scene::ChartGaps, 120, 30),
    ("dashboard-chart-idle.svg", Scene::ChartIdle, 120, 30),
    ("dashboard-chart-low-load.svg", Scene::ChartLowLoad, 120, 30),
    ("dashboard-chart-offline.svg", Scene::ChartOffline, 120, 30),
    ("dashboard-low-load.svg", Scene::LowLoad, 120, 30),
    ("dashboard-idle.svg", Scene::Idle, 120, 30),
    ("dashboard-reconnecting.svg", Scene::Reconnecting, 120, 30),
    (
        "dashboard-device-offline.svg",
        Scene::DeviceOffline,
        120,
        30,
    ),
    (
        "dashboard-daemon-offline.svg",
        Scene::DaemonOffline,
        120,
        30,
    ),
    (
        "dashboard-command-pending.svg",
        Scene::CommandPending,
        120,
        30,
    ),
    ("logs-modal.svg", Scene::Logs, 120, 30),
    ("logs-oldest-day.svg", Scene::LogsOldest, 120, 30),
    ("logs-modal-80x24.svg", Scene::Logs, 80, 24),
    ("settings-modal.svg", Scene::Settings, 120, 30),
    (
        "settings-theme-catppuccin.svg",
        Scene::SettingsTheme("Catppuccin"),
        120,
        30,
    ),
    (
        "settings-theme-nord.svg",
        Scene::SettingsTheme("Nord"),
        120,
        30,
    ),
    (
        "settings-theme-gruvbox.svg",
        Scene::SettingsTheme("Gruvbox"),
        120,
        30,
    ),
    (
        "settings-theme-tokyo-night.svg",
        Scene::SettingsTheme("Tokyo Night"),
        120,
        30,
    ),
    (
        "settings-theme-solarized.svg",
        Scene::SettingsTheme("Solarized"),
        120,
        30,
    ),
    (
        "settings-theme-terminal.svg",
        Scene::SettingsTheme("Terminal"),
        120,
        30,
    ),
    (
        "settings-theme-picker.svg",
        Scene::Picker(Field::Theme),
        120,
        30,
    ),
    ("settings-charts.svg", Scene::SettingsCharts, 120, 30),
    (
        "settings-interval-picker.svg",
        Scene::SettingsIntervalPicker,
        120,
        30,
    ),
    (
        "settings-visualization-picker.svg",
        Scene::SettingsVisualizationPicker,
        120,
        30,
    ),
    ("settings-alerts.svg", Scene::SettingsAlerts, 120, 30),
    ("settings-notify.svg", Scene::SettingsNotify, 120, 30),
    ("settings-debug.svg", Scene::SettingsDebug, 120, 30),
    ("settings-debug-60x19.svg", Scene::SettingsDebug, 60, 19),
    ("settings-charts-60x19.svg", Scene::SettingsCharts, 60, 19),
    ("help-settings.svg", Scene::SettingsHelp, 120, 30),
    ("help-modal.svg", Scene::Help, 120, 30),
    ("help-logs-modal.svg", Scene::LogsHelp, 120, 30),
    ("quit-modal.svg", Scene::Quit, 120, 30),
    ("terminal-too-small.svg", Scene::Live, 50, 14),
];

fn fixed_status(now: DateTime<Utc>) -> Result<Status, String> {
    serde_json::from_value(json!({
        "schema_version": 1,
        "server_time": now.to_rfc3339(),
        "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
        "state_version": 42,
        "device": { "name": "AP S300 V2.0" },
        "connection": {
            "phase": "connected", "desired": "running", "link_connected": true,
            "session_id": "fixed-session", "message": "Connected", "adapter_id": "hci2",
            "adapter_address": "F4:4E:FC:A1:CB:FF"
        },
        "telemetry": {
            "state": "live", "age_seconds": 0.4,
            "sample": {
                "sequence": 40, "received_at": now.to_rfc3339(), "segment_id": "fixed-segment",
                "battery_percent": 71, "input_power_w": 63, "output_power_w": 181,
                "remaining_minutes": 2937, "ac_enabled": true, "dc_enabled": false,
                "light_enabled": false
            }
        },
        "controls": { "allowed": true, "outputs_revision": 7, "pending_command_id": null, "reason_code": null },
        "history": { "state": "ok" },
        "logging": { "effective_level": "INFO", "override_expires_at": null }
    })).map_err(|error| error.to_string())
}

fn app(scene: Scene) -> Result<App, String> {
    if matches!(scene, Scene::LogsOldest) {
        let mut app = app(Scene::Logs)?;
        let items: Vec<_> = app.logs.records.iter().cloned().collect();
        let oldest = chrono::DateTime::parse_from_rfc3339(items[0]["timestamp"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        let request = app.logs.open().unwrap();
        app.logs.accept(
            &request,
            Ok(mypowers_tui::logs::Page {
                schema_version: 1,
                items,
                previous_cursor: None,
                next_cursor: None,
                has_more_before: false,
                has_more_after: false,
                source: "files".into(),
                gap: false,
                skipped_lines: 0,
                oldest_record: Some(Some(oldest)),
            }),
        );
        return Ok(app);
    }
    if let Scene::Themed(view, name) = scene {
        let base = match view {
            ThemeView::Dashboard | ThemeView::Small => Scene::Live,
            ThemeView::Chart => Scene::Chart,
            ThemeView::Logs => Scene::Logs,
            ThemeView::Help => Scene::Help,
            ThemeView::Quit => Scene::Quit,
        };
        let mut app = app(base)?;
        app.client_preferences.theme = name.into();
        return Ok(app);
    }

    let now: DateTime<Utc> = "2026-10-05T12:00:00Z"
        .parse()
        .map_err(|error: chrono::ParseError| error.to_string())?;
    let mut app = App::with_clock(
        false,
        Some("UTC".parse().unwrap()),
        Clock::Fixed {
            now,
            telemetry_elapsed: Duration::ZERO,
            animation_elapsed: Duration::from_secs(10),
            feedback_elapsed: Duration::from_secs(1),
        },
    );
    let mut status = fixed_status(now)?;
    app.server_url = "https://mypowers.lxc.efez.net".into();
    app.connected = true;
    app.selected = None;
    app.feedback = Some(Feedback::new("AC ON confirmed", Severity::Success));
    match scene {
        Scene::LogsOldest => unreachable!("handled before constructing the fixture"),
        Scene::Themed(_, _) => unreachable!("handled before constructing the fixture"),
        Scene::Snapshot => {
            app.snapshot = true;
            app.feedback = None;
            app.graph.visualization = Visualization::Chart;
            app.graph.resolution = Resolution::ThirtySeconds;
            status.telemetry.sample.as_mut().unwrap().dc_enabled = true;
        }
        Scene::ChartNearby => {
            let sample = status.telemetry.sample.as_mut().unwrap();
            sample.input_power_w = 51;
            sample.output_power_w = 28;
            app.feedback = None;
        }
        Scene::LowLoad | Scene::ChartLowLoad => {
            let sample = status.telemetry.sample.as_mut().unwrap();
            sample.input_power_w = 35;
            sample.output_power_w = 3;
            app.feedback = None;
        }
        Scene::Idle | Scene::ChartIdle => {
            let sample = status.telemetry.sample.as_mut().unwrap();
            sample.input_power_w = 0;
            sample.output_power_w = 0;
            app.feedback = None;
        }
        Scene::Reconnecting | Scene::DeviceOffline => {
            let reconnecting = matches!(scene, Scene::Reconnecting);
            status.connection.phase = if reconnecting {
                "reconnecting"
            } else {
                "backoff"
            }
            .into();
            status.connection.link_connected = false;
            status.connection.session_id = None;
            status.telemetry.state = "waiting".into();
            status.telemetry.sample = None;
            status.telemetry.age_seconds = None;
            status.controls.allowed = false;
            status.controls.reason_code = Some("telemetry_unavailable".into());
            if reconnecting {
                app.warning_count = 1;
            } else {
                app.error_count = 1;
            }
            app.feedback = Some(Feedback::new(
                if reconnecting {
                    "Reconnecting to station"
                } else {
                    "Station unavailable"
                },
                if reconnecting {
                    Severity::Warning
                } else {
                    Severity::Error
                },
            ));
        }
        Scene::DaemonOffline | Scene::ChartOffline => {
            app.connected = false;
            status.telemetry.state = "stale".into();
            status.telemetry.age_seconds = Some(45.0);
            status.controls.allowed = false;
            app.warning_count = 1;
            app.error_count = 1;
            app.feedback = Some(Feedback::new(
                "Connection lost; reconnecting to daemon",
                Severity::Error,
            ));
        }
        Scene::CommandPending => {
            status.telemetry.sample.as_mut().unwrap().ac_enabled = false;
            status.controls.allowed = false;
            status.controls.pending_command_id =
                Some("de719058-404d-45e7-97f3-9c234df154e1".into());
            app.pending = Some("AC -> ON".into());
            app.selected = Some(0);
            app.feedback = Some(Feedback::new(
                "Waiting for AC ON confirmation...",
                Severity::Info,
            ));
        }
        Scene::Logs | Scene::LogsLevel(_) => {
            app.view = View::Logs;
            app.logs.offset = 5;
            app.logs.unseen = 7;
            app.logs.message = "Logs loaded".into();
            let entries = [
                ("INFO", "Reconnecting to station."),
                ("INFO", "Scanning for station."),
                ("INFO", "Connected to station."),
                ("DEBUG", "Received station frame."),
                ("INFO", "Output operation accepted."),
                ("INFO", "Output operation completed."),
                ("WARNING", "Station telemetry delayed."),
                ("ERROR", "Station command not confirmed."),
                ("INFO", "Runtime log level changed."),
                ("INFO", "Station telemetry restored."),
            ];
            for index in 0..48 {
                let (level, message) = entries[index % entries.len()];
                app.logs.records.push_back(json!({
                    "timestamp": (now - TimeDelta::seconds(96 - index as i64 * 2)).to_rfc3339(),
                    "level": level, "message": message, "sequence": index,
                    "server_instance_id": status.server_instance_id,
                }));
            }
            status.logging =
                json!({"effective_level":"DEBUG", "override_expires_at":"2026-10-05T12:15:00Z"});
            app.feedback = Some(Feedback::new("Logs loaded", Severity::Info));
            if let Scene::LogsLevel(level) = scene {
                app.logs.level = level;
                app.logs.records.retain(|record| {
                    mypowers_tui::logs::LEVELS
                        .iter()
                        .position(|value| Some(*value) == record["level"].as_str())
                        .is_some_and(|index| index >= level)
                });
                app.logs.offset = 0;
            }
        }
        Scene::Settings
        | Scene::SettingsCharts
        | Scene::SettingsIntervalPicker
        | Scene::SettingsVisualizationPicker
        | Scene::SettingsAlerts
        | Scene::SettingsNotify
        | Scene::SettingsDebug
        | Scene::SettingsTheme(_)
        | Scene::SettingsHelp
        | Scene::Picker(_)
        | Scene::SettingsHelpTab(_)
        | Scene::NotifyState(_)
        | Scene::DebugState(_) => {
            app.view = View::Settings;
            app.settings = Some(Settings {
                schema_version: 1,
                graph_interval_seconds: 60,
                timezone: "UTC".into(),
                ..Settings::default()
            });
            app.settings_draft = app.settings.as_ref().unwrap().clone();
            if let Scene::SettingsTheme(name) = scene {
                app.client_preferences.theme = name.into();
            }
            app.settings_tab = match scene {
                Scene::SettingsCharts
                | Scene::SettingsIntervalPicker
                | Scene::SettingsVisualizationPicker
                | Scene::Picker(Field::Scale | Field::Visualization | Field::Interval) => {
                    SettingsTab::Charts
                }
                Scene::SettingsAlerts
                | Scene::Picker(
                    Field::AlertEnabled
                    | Field::AlertThreshold
                    | Field::AlertHysteresis
                    | Field::AlertCooldown,
                ) => SettingsTab::Alerts,
                Scene::SettingsNotify | Scene::NotifyState(_) => SettingsTab::Notify,
                Scene::SettingsDebug | Scene::SettingsHelp | Scene::DebugState(_) => {
                    SettingsTab::Debug
                }
                Scene::SettingsHelpTab(tab) => tab,
                _ => SettingsTab::Preferences,
            };
            if matches!(scene, Scene::SettingsIntervalPicker) {
                app.settings_picker = Some(mypowers_tui::settings::Picker {
                    field: mypowers_tui::settings::Field::Interval,
                    selected: 0,
                    query: String::new(),
                });
            }
            if matches!(scene, Scene::SettingsHelp) {
                app.view = View::Help;
                app.help_context = View::Settings;
            }
            app.warning_count = 1;
            app.feedback = Some(Feedback::new("Settings saved", Severity::Success));
            if let Scene::Picker(field) = scene {
                if field.is_segmented() {
                    let fields: &[Field] = match app.settings_tab {
                        SettingsTab::Charts => &Field::CHARTS,
                        SettingsTab::Alerts => &Field::ALERTS,
                        _ => &Field::PREFERENCES,
                    };
                    app.settings_selected = fields
                        .iter()
                        .position(|candidate| *candidate == field)
                        .unwrap();
                } else {
                    let selected = field
                        .choices()
                        .iter()
                        .position(|value| *value == app.settings_draft.value(field))
                        .unwrap_or(0);
                    app.settings_picker = Some(mypowers_tui::settings::Picker {
                        field,
                        selected,
                        query: String::new(),
                    });
                }
            }
            if let Scene::SettingsHelpTab(_) = scene {
                app.view = View::Help;
                app.help_context = View::Settings;
            }
            if let Scene::NotifyState(state) = scene {
                app.settings.as_mut().unwrap().telegram_configured = true;
                app.settings_draft.telegram_configured = true;
                app.warning_count = 0;
                app.feedback = match state {
                    NotifyState::Configured => None,
                    NotifyState::Sending => {
                        app.pending = Some("telegram test".into());
                        Some(Feedback::new("Sending Telegram test...", Severity::Info))
                    }
                    NotifyState::Sent => {
                        Some(Feedback::new("Telegram test sent", Severity::Success))
                    }
                    NotifyState::Failed => Some(Feedback::new(
                        "Telegram test failed; see server logs/configuration",
                        Severity::Error,
                    )),
                };
            }
            if let Scene::DebugState(paused) = scene {
                app.feedback = None;
                app.warning_count = 0;
                if paused {
                    status.connection.desired = "paused".into();
                    status.connection.phase = "paused".into();
                    status.connection.link_connected = false;
                    status.telemetry.state = "stale".into();
                    status.telemetry.age_seconds = Some(45.0);
                    status.controls.allowed = false;
                } else {
                    status.logging = json!({"effective_level":"DEBUG", "override_expires_at":"2026-10-05T12:15:00Z"});
                }
            }
        }
        Scene::Help | Scene::LogsHelp => {
            app.view = View::Help;
            app.help_context = if matches!(scene, Scene::LogsHelp) {
                View::Logs
            } else {
                View::Dashboard
            };
            app.feedback = None;
        }
        Scene::Quit => {
            app.view = View::Quit;
            app.feedback = None;
        }
        Scene::Live | Scene::Chart | Scene::ChartGaps => {}
        Scene::LiveMinute | Scene::ChartMinute => app.graph.resolution = Resolution::Minute,
        Scene::LiveHour | Scene::ChartHour => app.graph.resolution = Resolution::Hour,
        Scene::ThirtySeconds(chart) => {
            app.graph.resolution = Resolution::ThirtySeconds;
            if chart {
                app.graph.visualization = Visualization::Chart;
            }
        }
    }
    if matches!(
        scene,
        Scene::Chart
            | Scene::ChartMinute
            | Scene::ChartHour
            | Scene::ChartNearby
            | Scene::ChartGaps
            | Scene::ChartIdle
            | Scene::ChartLowLoad
            | Scene::ChartOffline
    ) {
        app.graph.visualization = Visualization::Chart;
    }
    if !status.valid() {
        return Err("Invalid snapshot status fixture".into());
    }
    if let Some(sample) = &status.telemetry.sample {
        let input = [12, 18, 32, 48, 67, 83, 72, 57, 41, 29, 20, 16];
        let output = [38, 52, 84, 113, 164, 218, 256, 229, 197, 146, 97, 63];
        let count = if app.graph.visualization == Visualization::Chart {
            90
        } else {
            44
        };
        for index in 0..count {
            let timestamp =
                now - TimeDelta::seconds((count - 1 - index) * app.graph.resolution.seconds());
            if matches!(scene, Scene::ChartGaps) && (index < 30 || (50..60).contains(&index)) {
                continue;
            }
            let flat = matches!(
                scene,
                Scene::Idle | Scene::LowLoad | Scene::ChartIdle | Scene::ChartLowLoad
            );
            app.graph.points.push(Point {
                bucket_start_ms: timestamp.timestamp_millis(),
                input_power_w: if matches!(scene, Scene::ChartNearby) {
                    if index % 2 == 0 { 49.0 } else { 51.0 }
                } else if flat {
                    sample.input_power_w as f64
                } else {
                    input[index as usize % input.len()] as f64
                },
                output_power_w: if matches!(scene, Scene::ChartNearby) {
                    28.0
                } else if flat {
                    sample.output_power_w as f64
                } else {
                    output[index as usize % output.len()] as f64
                },
                sample_count: (app.graph.resolution.seconds() / 10) as u64,
            });
        }
        let latest = app.graph.points.last_mut().unwrap();
        latest.input_power_w = sample.input_power_w as f64;
        latest.output_power_w = sample.output_power_w as f64;
    }
    app.status = Some(status);
    Ok(app)
}

/// Only views that can truthfully display a captured live state.
pub fn live_gallery_scene(scene: Scene) -> bool {
    !matches!(
        scene,
        Scene::LogsOldest
            | Scene::ChartNearby
            | Scene::ChartGaps
            | Scene::ChartIdle
            | Scene::ChartLowLoad
            | Scene::ChartOffline
            | Scene::LowLoad
            | Scene::Idle
            | Scene::Reconnecting
            | Scene::DeviceOffline
            | Scene::DaemonOffline
            | Scene::CommandPending
            | Scene::NotifyState(_)
            | Scene::DebugState(_)
    )
}

pub fn render_captured(
    scene: Scene,
    width: u16,
    height: u16,
    data: &serde_json::Value,
) -> Result<Buffer, String> {
    let mut app = app(scene)?;
    let status: Status =
        serde_json::from_value(data["status"].clone()).map_err(|e| e.to_string())?;
    if !status.valid() {
        return Err("Invalid captured status".into());
    }
    let now = DateTime::parse_from_rfc3339(&status.server_time)
        .map_err(|e| e.to_string())?
        .with_timezone(&Utc);
    let settings: Settings =
        serde_json::from_value(data["settings"].clone()).map_err(|e| e.to_string())?;
    if !settings.valid() {
        return Err("Invalid captured settings".into());
    }
    app.clock = Clock::Fixed {
        now,
        telemetry_elapsed: Duration::ZERO,
        animation_elapsed: Duration::from_secs(10),
        feedback_elapsed: Duration::ZERO,
    };
    app.timezone = settings.timezone.parse().ok();
    app.graph_base_scale_w = settings.graph_base_scale_w;
    let graph_scene = match scene {
        Scene::Themed(ThemeView::Dashboard | ThemeView::Small, _) => Scene::Live,
        Scene::Themed(ThemeView::Chart, _) => Scene::Chart,
        _ => scene,
    };
    app.graph.resolution = match graph_scene {
        Scene::Live | Scene::Chart => Resolution::TenSeconds,
        Scene::LiveMinute | Scene::ChartMinute => Resolution::Minute,
        Scene::LiveHour | Scene::ChartHour => Resolution::Hour,
        Scene::ThirtySeconds(_) => Resolution::ThirtySeconds,
        _ => settings.resolution().ok_or("Invalid graph interval")?,
    };
    app.graph.visualization = if matches!(
        graph_scene,
        Scene::Live | Scene::LiveMinute | Scene::LiveHour | Scene::ThirtySeconds(false)
    ) {
        Visualization::Sparkline
    } else {
        Visualization::Chart
    };
    app.settings = Some(settings.clone());
    app.settings_draft = settings;
    if let Some(picker) = &mut app.settings_picker {
        picker.selected = picker
            .field
            .choices()
            .iter()
            .position(|value| *value == app.settings_draft.value(picker.field))
            .unwrap_or(0);
    }
    app.feedback = None;
    app.warning_count = 0;
    app.error_count = 0;
    app.graph.points = serde_json::from_value(
        data["history"][app.graph.resolution.seconds().to_string()]["items"].clone(),
    )
    .map_err(|e| e.to_string())?;
    let level = app.logs.level;
    app.logs = mypowers_tui::logs::Logs::new_at(app.timezone, now);
    app.logs.level = level;
    if let Some(records) = data["logs"]["items"].as_array() {
        for record in records {
            if mypowers_tui::logs::LEVELS
                .iter()
                .position(|v| Some(*v) == record["level"].as_str())
                .is_some_and(|i| i >= level)
            {
                app.logs.records.push_back(record.clone());
            }
        }
    }
    if let Some(value) = data.get("logs_oldest") {
        let boundary: mypowers_tui::logs::Page =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if boundary.schema_version != 1 || boundary.items.len() > 1 {
            return Err("Invalid captured oldest log page".into());
        }
        let oldest = boundary
            .items
            .first()
            .map(|record| {
                DateTime::parse_from_rfc3339(
                    record["timestamp"]
                        .as_str()
                        .ok_or("Missing oldest log timestamp")?,
                )
                .map(|stamp| stamp.with_timezone(&Utc))
                .map_err(|_| "Invalid oldest log timestamp")
            })
            .transpose()?;
        let mut page: mypowers_tui::logs::Page =
            serde_json::from_value(data["logs"].clone()).map_err(|e| e.to_string())?;
        page.items = app.logs.records.iter().cloned().collect();
        page.oldest_record = Some(oldest);
        let request = app
            .logs
            .open()
            .ok_or("Cannot initialize captured log bounds")?;
        app.logs.accept(&request, Ok(page));
    }
    app.status = Some(status);
    draw_app(app, width, height)
}

fn draw_app(mut app: App, width: u16, height: u16) -> Result<Buffer, String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).map_err(|e| e.to_string())?;
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .map_err(|e| e.to_string())?;
    Ok(terminal.backend().buffer().clone())
}

pub fn render(scene: Scene, width: u16, height: u16) -> Result<Buffer, String> {
    let mut app = app(scene)?;
    let mut terminal =
        Terminal::new(TestBackend::new(width, height)).map_err(|error| error.to_string())?;
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .map_err(|error| error.to_string())?;
    Ok(terminal.backend().buffer().clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svg;

    fn text(buffer: &Buffer) -> String {
        buffer
            .content
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn captured_gallery_uses_capture_instead_of_fixture_readings() {
        let now: DateTime<Utc> = "2026-10-06T09:48:00Z".parse().unwrap();
        let mut status = fixed_status(now).unwrap();
        let sample = status.telemetry.sample.as_mut().unwrap();
        sample.battery_percent = 99;
        sample.input_power_w = 42;
        sample.output_power_w = 9;
        let settings = Settings {
            graph_interval_seconds: 30,
            graph_visualization: Visualization::Chart,
            timezone: "Europe/Warsaw".into(),
            ..Settings::default()
        };
        let data = json!({"status":status,"settings":settings,"history":{"30":{"items":[]}},"logs":{"items":[]}});
        let buffer = render_captured(Scene::ThirtySeconds(true), 98, 31, &data).unwrap();
        let content = text(&buffer);
        assert!(content.contains("99%"));
        assert!(content.contains("42") && content.contains("9"));
        assert!(!content.contains("AC ON confirmed"));
        assert_eq!(
            svg::export(&buffer).unwrap(),
            svg::export(&render_captured(Scene::ThirtySeconds(true), 98, 31, &data).unwrap())
                .unwrap()
        );
        assert!(!live_gallery_scene(Scene::ChartLowLoad));
        assert!(!live_gallery_scene(Scene::NotifyState(NotifyState::Sent)));
        assert!(live_gallery_scene(Scene::SettingsNotify));
    }

    #[test]
    fn every_scene_is_byte_identical_across_fresh_states_and_has_no_zero_counters() {
        for &(name, scene, width, height) in SCENES.iter().chain(GALLERY.iter()) {
            let first = render(scene, width, height).unwrap();
            let second = render(scene, width, height).unwrap();
            assert_eq!(svg::export(&first), svg::export(&second), "{name}");
            let text = text(&first);
            assert!(
                !text.contains("warn:0") && !text.contains("err:0"),
                "{name}"
            );
            assert!(
                !text.contains("2026-10-05T12:00:00Z"),
                "raw server timestamp: {name}"
            );
        }
    }

    #[test]
    fn scenes_exercise_real_widgets_contexts_and_status_feedback() {
        let idle = text(&render(Scene::Idle, 120, 30).unwrap());
        assert!(idle.contains("INPUT 0 W") && idle.contains("OUTPUT 0 W"));
        assert_eq!(idle.matches('○').count(), 2);
        let live = text(&render(Scene::Live, 80, 24).unwrap());
        assert!(
            live.contains("71%") && live.contains("INPUT 63 W") && live.contains("OUTPUT 181 W")
        );
        assert!(live.contains("CONNECTED") && live.contains("AC ON confirmed"));
        let chart = render(Scene::Chart, 120, 30).unwrap();
        let chart_text = text(&chart);
        assert!(chart_text.contains("400") && chart_text.contains("200"));
        assert!(!chart_text.contains("0–400 W"));
        for color in [
            ratatui::style::Color::Rgb(118, 203, 137),
            ratatui::style::Color::Rgb(92, 181, 204),
        ] {
            assert!(chart.content.iter().any(|cell| {
                cell.fg == color
                    && cell
                        .symbol()
                        .chars()
                        .any(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
            }));
        }
        assert_eq!(
            text(&render(Scene::ChartIdle, 120, 30).unwrap())
                .matches('○')
                .count(),
            1
        );
        let reconnecting = text(&render(Scene::Reconnecting, 120, 30).unwrap());
        assert!(
            reconnecting.contains("--%")
                && reconnecting.contains("INPUT -- W")
                && reconnecting.contains("warn:1")
        );
        let offline = text(&render(Scene::DaemonOffline, 120, 30).unwrap());
        assert!(offline.contains("DAEMON OFFLINE") && offline.contains("warn:1 • err:1"));
        let pending = text(&render(Scene::CommandPending, 120, 30).unwrap());
        assert!(pending.contains("AC -> ON") && pending.contains("Waiting for AC ON confirmation"));
        assert!(!pending.contains("de719058"));
        let logs = text(&render(Scene::Logs, 120, 30).unwrap());
        assert!(
            logs.contains(" LOGS ")
                && logs.contains("AP S300 V2.0")
                && logs.contains("Log: DEBUG | until 2026-10-05 12:15:00")
        );
        let settings = text(&render(Scene::SettingsDebug, 120, 30).unwrap());
        assert!(
            settings.contains(" SETTINGS ")
                && settings.contains("Debug / Diagnostics")
                && settings.contains("hci2")
        );
        let charts = text(&render(Scene::SettingsCharts, 60, 19).unwrap());
        assert!(charts.contains("Interval per bar") && charts.contains("60s"));
        let footer = charts
            .lines()
            .find(|row| row.contains("Enter choose"))
            .unwrap();
        assert!(footer.starts_with('╰') && footer.ends_with('╯'));
        let alerts = text(&render(Scene::SettingsAlerts, 60, 19).unwrap());
        assert!(
            alerts.contains("Low threshold") && alerts.contains("20%") && alerts.contains("5 pp")
        );
        assert!(
            text(&render(Scene::SettingsNotify, 60, 19).unwrap()).contains("Send test message")
        );
        let help = text(&render(Scene::Help, 120, 30).unwrap());
        assert!(help.contains(" HELP ") && help.contains("AP S300 V2.0"));
        assert!(text(&render(Scene::Live, 50, 14).unwrap()).contains("Terminal too small"));
    }
}
