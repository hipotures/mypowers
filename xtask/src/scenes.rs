//! Fixed data only. Layout and widgets live exclusively in the production TUI crate.
use chrono::{DateTime, Duration as TimeDelta, Utc};
use mypowers_tui::{
    app::{App, View},
    clock::Clock,
    feedback::{Feedback, Severity},
    history::{Point, Resolution, Visualization},
    model::Status,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use serde_json::json;
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum Scene {
    Live,
    LiveMinute,
    LiveHour,
    Chart,
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
    Settings,
    Help,
    LogsHelp,
    Quit,
}

pub const SCENES: &[(&str, Scene, u16, u16)] = &[
    ("dashboard-live.svg", Scene::Live, 120, 30),
    ("dashboard-live-60s.svg", Scene::LiveMinute, 120, 30),
    ("dashboard-live-1h.svg", Scene::LiveHour, 120, 30),
    ("dashboard-live-80x24.svg", Scene::Live, 80, 24),
    ("dashboard-live-60x19.svg", Scene::Live, 60, 19),
    ("dashboard-chart.svg", Scene::Chart, 120, 30),
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
    ("logs-modal-80x24.svg", Scene::Logs, 80, 24),
    ("settings-modal.svg", Scene::Settings, 120, 30),
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
            "session_id": "fixed-session", "message": "Connected", "adapter_id": "hci2"
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
    app.connected = true;
    app.selected = None;
    app.feedback = Some(Feedback::new("AC ON confirmed", Severity::Success));
    match scene {
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
        Scene::Logs => {
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
        }
        Scene::Settings => {
            app.view = View::Settings;
            app.warning_count = 1;
            app.feedback = Some(Feedback::new("Diagnostics loaded", Severity::Info));
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
        Scene::LiveMinute => app.graph.resolution = Resolution::Minute,
        Scene::LiveHour => app.graph.resolution = Resolution::Hour,
    }
    if matches!(
        scene,
        Scene::Chart
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
                input_power_w: if flat {
                    sample.input_power_w as f64
                } else {
                    input[index as usize % input.len()] as f64
                },
                output_power_w: if flat {
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
    fn every_scene_is_byte_identical_across_fresh_states_and_has_no_zero_counters() {
        for &(name, scene, width, height) in SCENES {
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
        assert_eq!(chart_text.matches("0–400 W").count(), 1);
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
        let settings = text(&render(Scene::Settings, 120, 30).unwrap());
        assert!(
            settings.contains(" SETTINGS ")
                && settings.contains("Debug / Diagnostics")
                && settings.contains("hci2")
        );
        let help = text(&render(Scene::Help, 120, 30).unwrap());
        assert!(help.contains(" HELP ") && help.contains("AP S300 V2.0"));
        assert!(text(&render(Scene::Live, 50, 14).unwrap()).contains("Terminal too small"));
    }
}
