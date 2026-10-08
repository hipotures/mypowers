use crate::{
    app::{App, Effect, View},
    feedback::{Feedback, Severity},
    history::{Point, Resolution},
    model::{Command, Status},
    network::{ClipboardTarget, Event, Intent},
    settings::{Settings, SettingsTab},
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use serde_json::json;
use std::time::{Duration, Instant};

pub(crate) fn status() -> Status {
    let stamp = chrono::Utc::now().to_rfc3339();
    serde_json::from_value(json!({
        "schema_version":1,"server_time":stamp,"server_instance_id":"88767477-2a2a-481f-843b-30d56a5e3f10","state_version":1,
        "device":{"name":"AP S300 V2.0"},
        "connection":{"phase":"connected","desired":"running","link_connected":true,"session_id":"session","message":"Connected","adapter_id":"hci0","adapter_address":"A8:3B:76:E6:D4:A0"},
        "telemetry":{"state":"live","age_seconds":0.0,"sample":{
            "sequence":1,"received_at":stamp,"segment_id":"segment",
            "battery_percent":78,"input_power_w":63,"output_power_w":181,"remaining_minutes":2937,
            "ac_enabled":false,"dc_enabled":false,"light_enabled":false}},
        "controls":{"allowed":true,"outputs_revision":2,"pending_command_id":null,"reason_code":null},
        "history":{"state":"ok"},"logging":{"effective_level":"INFO"}
    })).unwrap()
}

fn app() -> App {
    let mut app = App::new(false, None);
    app.update(Event::Status(Box::new(status())));
    seed_current_graph(&mut app);
    app
}

fn seed_current_graph(app: &mut App) {
    let sample = app
        .status
        .as_ref()
        .unwrap()
        .telemetry
        .sample
        .as_ref()
        .unwrap();
    let span = app.graph.resolution.seconds() * 1000;
    app.graph.points = vec![Point {
        bucket_start_ms: app.timeline_now_ms().div_euclid(span) * span,
        input_power_w: sample.input_power_w as f64,
        output_power_w: sample.output_power_w as f64,
        sample_count: 1,
    }];
}

fn fixed_graph_app() -> App {
    use crate::clock::Clock;
    let now = "2026-10-05T12:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let mut app = App::with_clock(
        false,
        Some(chrono_tz::UTC),
        Clock::Fixed {
            now,
            telemetry_elapsed: Duration::ZERO,
            animation_elapsed: Duration::ZERO,
            feedback_elapsed: Duration::ZERO,
        },
    );
    let mut current = status();
    current.server_time = now.to_rfc3339();
    current.telemetry.sample.as_mut().unwrap().received_at = now.to_rfc3339();
    app.update(Event::Status(Box::new(current)));
    app
}

fn render(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn text(buffer: &Buffer) -> String {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn settings_tabs_wrap_route_contextual_keys_and_mouse_without_dashboard_actions() {
    let mut app = app();
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    for tab in SettingsTab::ALL {
        assert_eq!(app.settings_tab, tab);
        let screen = text(&render(&mut app, 60, 19));
        assert!(
            SettingsTab::ALL
                .iter()
                .all(|tab| screen.contains(tab.title()))
        );
        assert!(app.controls.iter().all(|rect| rect.width == 0));
        for key in ['a', 'l'] {
            assert!(matches!(
                app.key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
                Effect::None
            ));
        }
        if tab != SettingsTab::Debug {
            for key in ['r', 'p', 'b'] {
                assert!(matches!(
                    app.key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
                    Effect::None
                ));
            }
        }
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }
    assert_eq!(app.settings_tab, SettingsTab::Preferences);
    app.key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.settings_tab, SettingsTab::Debug);
    render(&mut app, 94, 29);
    let target = app.settings_tabs[1];
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: target.x,
        row: target.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.settings_tab, SettingsTab::Charts);
    app.key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    let help = text(&render(&mut app, 94, 29));
    assert!(help.contains("HELP — SETTINGS / Charts") && help.contains("confirm"));
    assert!(!help.contains("Retry station connection"));
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.view == View::Dashboard);
    assert!(app.settings_tabs.iter().all(|rect| rect.width == 0));
    let selected = app.selected;
    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_ne!(app.selected, selected);
}

#[test]
fn battery_alert_settings_and_telegram_test_work_at_minimum_size() {
    let mut app = app();
    app.update(Event::Settings(Settings::default()));
    app.view = View::Settings;
    app.settings_tab = SettingsTab::Alerts;
    let screen = text(&render(&mut app, 60, 19));
    for label in [
        "Battery alert",
        "Low threshold",
        "20%",
        "Hysteresis",
        "5 pp",
        "10 min",
    ] {
        assert!(screen.contains(label), "Missing {label}");
    }
    app.settings_selected = 1;
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.settings_draft.battery_alert.threshold_percent, 21);
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Effect::Request(Intent::SaveSettings(_))
    ));
    app.update(Event::Finished(Feedback::new(
        "Settings saved",
        Severity::Success,
    )));
    app.view = View::Settings;
    app.settings_tab = SettingsTab::Notify;
    app.settings_selected = 0;
    let screen = text(&render(&mut app, 60, 19));
    assert!(screen.contains("Not configured") && screen.contains("Send test message"));
    let rect = app.settings_actions[0];
    assert!(matches!(
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE
        }),
        Effect::None
    ));
    assert!(matches!(
        app.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE
        }),
        Effect::Request(Intent::TestTelegram)
    ));
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Effect::None
    ));
}

#[test]
fn settings_form_lists_values_saves_every_field_and_preserves_failed_drafts() {
    let mut app = app();
    let original = Settings {
        graph_interval_seconds: 60,
        ..Settings::default()
    };
    app.update(Event::Settings(original.clone()));
    assert_eq!(app.graph.resolution, Resolution::Minute);
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let picker = text(&render(&mut app, 60, 19));
    assert!(picker.contains("Sparkline") && picker.contains("Chart"));
    assert!(app.settings_picker.is_none());
    assert_eq!(
        app.settings_draft.graph_visualization,
        crate::history::Visualization::Chart
    );
    assert_eq!(
        app.graph.visualization,
        crate::history::Visualization::Sparkline
    );
    app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let choices = text(&render(&mut app, 60, 19));
    for choice in ["10s", "30s", "60s", "1h"] {
        assert!(choices.contains(choice));
    }
    app.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.settings_draft.graph_interval_seconds, 30);
    app.settings_draft.graph_base_scale_w = 300;
    app.settings_draft.timezone = "Europe/Warsaw".into();
    app.settings_draft.logs_page_size = 250;
    let Effect::Request(Intent::SaveSettings(draft)) =
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    else {
        panic!("Expected complete settings save");
    };
    assert_eq!(draft, app.settings_draft);
    app.update(Event::Finished(Feedback::new(
        "Could not save settings; try again",
        Severity::Error,
    )));
    assert_eq!(app.settings, Some(original));
    assert_eq!(app.settings_draft, draft);
    app.settings_save_due = Some(app.clock.now());
    assert!(matches!(
        app.autosave(),
        Effect::Request(Intent::SaveSettings(_))
    ));
    app.update(Event::Settings(draft.clone()));
    app.update(Event::Finished(Feedback::new(
        "Settings saved",
        Severity::Success,
    )));
    assert_eq!(app.settings, Some(draft.clone()));
    assert_eq!(app.graph.resolution, Resolution::ThirtySeconds);
    assert_eq!(
        app.graph.visualization,
        crate::history::Visualization::Chart
    );
    assert_eq!(app.graph_base_scale_w, 300);
    assert_eq!(app.logs.page_size, 250);
    assert_eq!(app.timezone, Some(chrono_tz::Europe::Warsaw));
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    render(&mut app, 94, 29);
    assert!(matches!(app.autosave(), Effect::None));
    assert!(!text(&render(&mut app, 94, 29)).contains("Save changes"));
    let mut late = fixed_graph_app();
    late.key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    late.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    late.update(Event::Settings(Settings::default()));
    assert_eq!(late.graph.resolution, Resolution::ThirtySeconds);
    assert_eq!(
        late.graph.visualization,
        crate::history::Visualization::Chart
    );
}

#[test]
fn settings_buttons_require_release_and_old_hotkeys_do_not_dispatch() {
    let mut app = app();
    app.view = View::Settings;
    app.settings_tab = SettingsTab::Debug;
    for code in ['r', 'p', 'b', 'g', 't', 'd'] {
        assert!(matches!(
            app.key(KeyEvent::new(KeyCode::Char(code), KeyModifiers::NONE)),
            Effect::None
        ));
        assert!(app.pending.is_none());
    }
    for index in 0..3 {
        render(&mut app, 60, 19);
        let rect = app.settings_actions[index];
        assert!(rect.height == 1 && rect.width > 0);
        let event = |kind| MouseEvent {
            kind,
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(matches!(
            app.mouse(event(MouseEventKind::Down(MouseButton::Left))),
            Effect::None
        ));
        let effect = app.mouse(event(MouseEventKind::Up(MouseButton::Left)));
        assert!(matches!(
            (index, effect),
            (0, Effect::Request(Intent::Retry))
                | (1, Effect::Request(Intent::Connection(false)))
                | (2, Effect::Request(Intent::Debug(true)))
        ));
        app.update(Event::Finished(Feedback::new("Completed", Severity::Info)));
    }
    let rect = app.settings_actions[0];
    let event = |kind| MouseEvent {
        kind,
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    app.mouse(event(MouseEventKind::Down(MouseButton::Left)));
    app.resize();
    assert!(matches!(
        app.mouse(event(MouseEventKind::Up(MouseButton::Left))),
        Effect::None
    ));
}

#[test]
fn persisted_preferences_apply_on_startup_and_timezone_editor_supports_search() {
    let mut app = app();
    let settings = Settings {
        graph_interval_seconds: 30,
        graph_visualization: crate::history::Visualization::Chart,
        graph_base_scale_w: 300,
        timezone: "Europe/Warsaw".into(),
        logs_page_size: 500,
        ..Settings::default()
    };
    app.update(Event::Settings(settings));
    assert_eq!(
        app.graph.visualization,
        crate::history::Visualization::Chart
    );
    assert_eq!(app.graph.resolution, Resolution::ThirtySeconds);
    assert_eq!(app.graph_base_scale_w, 300);
    assert_eq!(app.timezone, Some(chrono_tz::Europe::Warsaw));
    assert_eq!(app.logs.page_size, 500);
    assert_eq!(crate::history::power_scale_from(300, [301.0]), 600);
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for character in "UTC".chars() {
        app.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    assert!(text(&render(&mut app, 60, 19)).contains("Search: UTC"));
    let picker = app.settings_picker.as_ref().unwrap();
    assert!(
        picker
            .options()
            .iter()
            .all(|(_, name)| name.to_lowercase().contains("utc"))
    );
    let rect = app.settings_choices[0].0;
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.settings_picker.is_none());
    assert_eq!(app.settings_draft.timezone, "UTC");
    assert_eq!(
        app.timezone,
        Some(chrono_tz::Europe::Warsaw),
        "Unsaved choices must remain a draft"
    );
    let previous = app.logs.open().unwrap();
    app.logs
        .configure(Some(chrono_tz::UTC), 100, app.clock.now());
    let next = app.logs.open().unwrap();
    assert!(next.generation > previous.generation);
}

#[test]
fn safe_text_borrows_clean_input_and_preserves_sanitization_limits() {
    use crate::model::safe;
    use std::borrow::Cow;

    for clean in ["", "AC ON confirmed", "╭ ● ▁█ 界 e\u{0301} 🇵🇱 👨‍👩‍👧‍👦"]
    {
        let cleaned = safe(clean);
        assert!(matches!(cleaned, Cow::Borrowed(_)));
        assert_eq!(cleaned, clean);
        assert_eq!(cleaned.as_ptr(), clean.as_ptr());
    }
    for (input, expected) in [
        ("AC\nON\r\t\0OFF", "ACONOFF"),
        ("\x1b[31mWarning\x1b[0m", "[31mWarning[0m"),
        ("\x1b]52;c;payload\x07", "]52;c;payload"),
        ("A\u{0085}\u{009b}B\u{007f}", "AB"),
    ] {
        let cleaned = safe(input);
        assert!(matches!(cleaned, Cow::Owned(_)));
        assert_eq!(cleaned, expected);
        assert!(cleaned.chars().all(|character| !character.is_control()));
    }
    // The cap counts Unicode scalar values, not UTF-8 bytes or filtered controls.
    let boundary = "界".repeat(2000);
    assert!(matches!(safe(&boundary), Cow::Borrowed(_)));
    for input in [format!("{boundary}界"), format!("\0\n{boundary}\u{009b}界")] {
        let cleaned = safe(&input);
        assert!(matches!(cleaned, Cow::Owned(_)));
        assert_eq!(cleaned, boundary);
        assert_eq!(cleaned.chars().count(), 2000);
    }
}

#[test]
fn commands_capture_revision_without_optimistic_state_changes_or_replay() {
    let mut app = app();
    let Effect::Request(Intent::Output {
        output,
        enabled,
        snapshot,
        key,
    }) = app.toggle(0)
    else {
        panic!("Expected AC intent");
    };
    assert_eq!(output, "ac");
    assert!(enabled);
    assert_eq!(snapshot.controls.outputs_revision, 2);
    assert!(uuid::Uuid::parse_str(&key).is_ok());
    assert!(
        !app.status
            .as_ref()
            .unwrap()
            .telemetry
            .sample
            .as_ref()
            .unwrap()
            .ac_enabled
    );
    assert!(matches!(app.toggle(0), Effect::None));
    app.update(Event::Disconnected("Lost".into()));
    app.update(Event::Status(Box::new(status())));
    assert!(matches!(app.toggle(1), Effect::None));
    app.update(Event::Finished(Feedback::new(
        "Command outcome uncertain",
        Severity::Warning,
    )));
    assert!(
        !app.status
            .as_ref()
            .unwrap()
            .telemetry
            .sample
            .as_ref()
            .unwrap()
            .ac_enabled
    );
}

#[test]
fn freshness_is_monotonic_and_busy_unknown_and_stale_states_disable_controls() {
    let mut app = app();
    assert!(app.allowed());
    app.received = Instant::now() - Duration::from_secs(4);
    assert!(!app.allowed());
    let mut current = status();
    current.controls.allowed = false;
    app.update(Event::Status(Box::new(current)));
    assert!(!app.allowed());
    for state in ["stale", "invalid", "unknown", "waiting"] {
        let mut current = status();
        current.telemetry.state = state.into();
        app.update(Event::Status(Box::new(current)));
        assert!(!app.allowed());
    }
}

#[test]
fn layouts_preserve_inline_values_two_row_graphs_and_unknown_values() {
    for (width, height) in [(60, 19), (80, 24), (94, 24), (94, 28), (120, 40)] {
        let mut app = app();
        let buffer = render(&mut app, width, height);
        let screen = text(&buffer);
        assert!(screen.contains("INPUT 63 W") && screen.contains("OUTPUT 181 W"));
        assert!(screen.contains("78%") && screen.contains("48h 57m"));
        assert_eq!(screen.matches("INPUT").count(), 1);
        assert!(app.controls.iter().all(|rect| rect.height == 2));
        for pair in app.controls.windows(2) {
            assert!(pair[0].intersection(pair[1]).is_empty());
        }
        app.no_color = true;
        let buffer = render(&mut app, width, height);
        assert!(text(&buffer).contains("████"));
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| cell.fg == ratatui::style::Color::Reset
                    && cell.bg == ratatui::style::Color::Reset)
        );
        for view in [View::Logs, View::Help, View::Settings] {
            app.view = view;
            render(&mut app, width, height);
            assert!(app.controls.iter().all(|rect| rect.is_empty()));
        }
    }
    let mut unknown = App::new(false, None);
    let screen = text(&render(&mut unknown, 80, 24));
    assert!(screen.contains("--%") && screen.contains("--h --m"));
    assert!(!screen.contains("OFF "));
    render(&mut unknown, 59, 19);
    assert!(unknown.title.is_empty());
}

#[test]
fn repeated_resize_matches_fresh_frames_and_keeps_hitboxes_in_bounds() {
    use crate::clock::Clock;
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    for view in [
        View::Dashboard,
        View::Logs,
        View::Help,
        View::Settings,
        View::Quit,
    ] {
        let mut app = App::with_clock(
            false,
            Some(chrono_tz::UTC),
            Clock::Fixed {
                now,
                telemetry_elapsed: Duration::ZERO,
                animation_elapsed: Duration::ZERO,
                feedback_elapsed: Duration::ZERO,
            },
        );
        let mut current = status();
        current.server_time = "2026-10-05T12:00:00Z".into();
        current.telemetry.sample.as_mut().unwrap().received_at = current.server_time.clone();
        app.update(Event::Status(Box::new(current)));
        app.view = view;
        for sequence in 0..100 {
            app.logs.records.push_back(json!({
                "timestamp": "2026-10-05T12:00:00Z", "level": "INFO",
                "sequence": sequence,
                "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
                "message": format!("Record {sequence:03}")
            }));
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        for width in [0, 1, 2, 20, 59, 60, 61, 80, 94, 95, 120] {
            for height in [0, 1, 2, 10, 18, 19, 24, 28, 29, 40] {
                terminal.backend_mut().resize(width, height);
                app.resize();
                terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
                let actual = terminal.backend().buffer().clone();
                for hitbox in app
                    .controls
                    .iter()
                    .chain(&app.quit_buttons)
                    .chain(&app.logs.buttons)
                    .chain([
                        &app.title,
                        &app.logs.title,
                        &app.logs.scrollbar,
                        &app.logs.thumb,
                    ])
                    .filter(|rect| !rect.is_empty())
                {
                    assert_eq!(hitbox.intersection(actual.area), *hitbox);
                }
                if width < ui::MIN_WIDTH || height < ui::MIN_HEIGHT {
                    assert!(app.controls.iter().all(|rect| rect.is_empty()));
                    assert!(app.quit_buttons.iter().all(|rect| rect.is_empty()));
                    assert!(app.logs.buttons.iter().all(|rect| rect.is_empty()));
                    assert!(app.title.is_empty() && app.logs.title.is_empty());
                    assert!(app.logs.scrollbar.is_empty() && app.logs.thumb.is_empty());
                }
                assert_eq!(actual, render(&mut app, width, height));
            }
        }
    }
}

#[test]
fn view_transitions_clear_old_symbols_and_styles_in_a_reused_terminal() {
    use crate::clock::Clock;
    fn draw_frame(terminal: &mut Terminal<TestBackend>, app: &mut App) -> Buffer {
        let mut rendered = None;
        terminal
            .draw(|frame| {
                ui::draw(frame, app);
                rendered = Some(frame.buffer_mut().clone());
            })
            .unwrap();
        rendered.unwrap()
    }
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    let views = [
        View::Dashboard,
        View::Logs,
        View::Help,
        View::Settings,
        View::Quit,
    ];
    for (no_color, unicode) in [(false, false), (false, true), (true, false), (true, true)] {
        for (width, height) in [(60, 19), (80, 24), (94, 29), (120, 40)] {
            let mut app = App::with_clock(
                no_color,
                Some(chrono_tz::UTC),
                Clock::Fixed {
                    now,
                    telemetry_elapsed: Duration::ZERO,
                    animation_elapsed: Duration::from_secs(8),
                    feedback_elapsed: Duration::from_secs(1),
                },
            );
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut step = 0;
            for from in views {
                for to in views {
                    for view in [from, to] {
                        let long_text = step % 2 == 0;
                        let fragment = if unicode {
                            "界 e\u{0301} 👨‍👩‍👧‍👦"
                        } else {
                            "Long station text "
                        };
                        let mut current = status();
                        current.server_time = "2026-10-05T12:00:00Z".into();
                        current.device["name"] = json!(if long_text {
                            fragment.repeat(12)
                        } else {
                            "AP S300".to_owned()
                        });
                        let sample = current.telemetry.sample.as_mut().unwrap();
                        sample.received_at = current.server_time.clone();
                        sample.sequence = step;
                        sample.battery_percent = [78, 3, 100][step as usize % 3];
                        app.update(Event::Status(Box::new(current)));
                        if step % 3 == 0 {
                            app.update(Event::Disconnected("Connection lost".into()));
                        }
                        app.view = view;
                        app.help_context =
                            [View::Dashboard, View::Logs, View::Settings][step as usize % 3];
                        app.resize();
                        app.logs.records.clear();
                        if long_text {
                            for sequence in 0..40 {
                                app.logs.records.push_back(json!({
                                    "timestamp": "2026-10-05T12:00:00Z", "level": "ERROR",
                                    "sequence": sequence,
                                    "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
                                    "message": fragment.repeat(12)
                                }));
                            }
                        }
                        app.warning_count = u32::from(long_text);
                        app.error_count = u32::from(!long_text);
                        app.feedback = Some(Feedback::new(
                            if long_text {
                                fragment.repeat(20)
                            } else {
                                "Saved".into()
                            },
                            if long_text {
                                Severity::Error
                            } else {
                                Severity::Success
                            },
                        ));
                        let actual_frame = draw_frame(&mut terminal, &mut app);
                        let mut fresh = Terminal::new(TestBackend::new(width, height)).unwrap();
                        let fresh_frame = draw_frame(&mut fresh, &mut app);
                        assert_eq!(
                            actual_frame, fresh_frame,
                            "Full frame at {width}x{height}, no_color={no_color}, unicode={unicode}, step={step}"
                        );
                        // TestBackend does not emulate erasing a wide glyph's trailing cell.
                        // Unicode is checked in the complete pre-flush frame above.
                        if !unicode {
                            assert_eq!(
                                terminal.backend().buffer(),
                                fresh.backend().buffer(),
                                "{width}x{height}, no_color={no_color}, transition step={step}"
                            );
                        }
                        step += 1;
                    }
                }
            }
            assert_eq!(step, 50);
        }
    }
}

#[test]
fn status_strip_is_outside_border_fades_and_leaves_no_reassuring_noise() {
    let mut app = app();
    let mut colors = Vec::new();
    for seconds in [0, 2, 4, 6, 8] {
        let mut feedback = Feedback::new("AC ON confirmed", Severity::Success);
        feedback.started -= Duration::from_secs(seconds);
        app.feedback = Some(feedback);
        let buffer = render(&mut app, 94, 29);
        let row: String = (0..94).map(|x| buffer[(x, 28)].symbol()).collect();
        assert_eq!(buffer[(0, 27)].symbol(), "╰");
        assert!(text(&buffer).lines().nth(27).unwrap().contains("q quit"));
        if seconds < 8 {
            assert!(row.starts_with(" AC ON confirmed"));
            assert!(row[" AC ON confirmed".len()..].trim().is_empty());
            colors.push(buffer[(1, 28)].fg);
        } else {
            assert!(row.trim().is_empty());
        }
        assert!(!text(&buffer).contains("Age "));
        assert!(!text(&buffer).contains("History ok"));
        assert!(!text(&buffer).contains("hci0"));
        assert!(!text(&buffer).contains("Log INFO"));
    }
    assert!(colors.windows(2).all(|pair| pair[0] != pair[1]));
    app.feedback = Some(Feedback::new("New message", Severity::Info));
    let screen = text(&render(&mut app, 60, 19));
    let row = screen.lines().last().unwrap();
    assert!(row.starts_with(" New message"));
    assert_eq!(row.trim(), "New message");
    render(&mut app, 60, 18);
    assert!(app.title.is_empty());
}

#[test]
fn status_strip_reserves_right_indicators_and_ellipsizes_unicode_feedback() {
    use ratatui::{
        layout::Rect,
        style::{Color, Style},
        text::{Line, Span},
    };
    let mut terminal = Terminal::new(TestBackend::new(30, 1)).unwrap();
    let feedback = Feedback::new("Station connected 🔋 more text to truncate", Severity::Info);
    let indicators = Line::from(vec![
        Span::styled("warn:2", Style::default().fg(Color::Yellow)),
        Span::raw(" • "),
        Span::styled("err:1", Style::default().fg(Color::Red)),
    ]);
    terminal
        .draw(|frame| {
            ui::status_line(
                frame,
                Rect::new(0, 0, 30, 1),
                Some(&feedback),
                indicators.clone(),
                Duration::ZERO,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(text(buffer), "Station connec… warn:2 • err:1");
    assert_eq!(buffer[(15, 0)].symbol(), " ");
    assert_eq!(buffer[(16, 0)].fg, Color::Yellow);
    assert_eq!(buffer[(29, 0)].fg, Color::Red);
    let feedback = Feedback::new("🔋🔋🔋🔋🔋", Severity::Success);
    terminal
        .draw(|frame| {
            ui::status_line(
                frame,
                Rect::new(0, 0, 5, 1),
                Some(&feedback),
                Line::default(),
                Duration::ZERO,
            )
        })
        .unwrap();
    assert_eq!(terminal.backend().buffer()[(4, 0)].symbol(), "…");
}

#[test]
fn status_strip_truncation_preserves_complete_graphemes() {
    use ratatui::{layout::Rect, text::Line};
    for (message, width, expected) in [
        ("🇵🇱 connection restored", 2, "…"),
        ("🇵🇱 connection restored", 3, "🇵🇱…"),
        ("1\u{fe0f}\u{20e3} command confirmed", 2, "…"),
        (
            "👨\u{200d}👩\u{200d}👧\u{200d}👦 connected",
            3,
            "👨\u{200d}👩\u{200d}👧\u{200d}👦…",
        ),
        ("e\u{301} connection restored", 2, "e\u{301}…"),
        ("你好 connection restored", 4, "你…"),
        ("connected", 1, "…"),
        ("connected", 9, "connected"),
        ("connected", 0, ""),
    ] {
        let feedback = Feedback::new(message, Severity::Info);
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        terminal
            .draw(|frame| {
                ui::status_line(
                    frame,
                    Rect::new(0, 0, width, 1),
                    Some(&feedback),
                    Line::default(),
                    Duration::ZERO,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        // Wide graphemes have a blank continuation cell in the terminal buffer.
        assert_eq!(
            rendered.replace(' ', ""),
            expected,
            "{message:?} at {width} columns"
        );
    }
}

#[test]
fn operational_feedback_ignores_debug_old_logs_and_repeated_telemetry() {
    let mut app = app();
    app.feedback = Some(Feedback::new("AC ON confirmed", Severity::Success));
    let started = app.feedback.as_ref().unwrap().started;
    app.update(Event::Status(Box::new(status())));
    assert_eq!(app.feedback.as_ref().unwrap().started, started);
    for (sequence, level, timestamp) in [
        (1, "DEBUG", chrono::Utc::now()),
        (2, "INFO", chrono::Utc::now() - chrono::Duration::minutes(1)),
    ] {
        app.update(Event::Log(json!({
            "server_instance_id":"88767477-2a2a-481f-843b-30d56a5e3f10",
            "sequence":sequence, "level":level, "timestamp":timestamp.to_rfc3339(),
            "message":"Must remain only in Logs",
        })));
    }
    assert_eq!(app.feedback.as_ref().unwrap().started, started);
    assert_eq!(app.logs.records.len(), 0);
    let record = json!({
        "server_instance_id":"88767477-2a2a-481f-843b-30d56a5e3f10",
        "sequence":3,"level":"INFO", "timestamp":chrono::Utc::now().to_rfc3339(),
        "event":"log_level_changed", "message":"Runtime log level changed.",
        "context":{"effective":"DEBUG"},
    });
    app.update(Event::Log(record.clone()));
    assert_eq!(
        app.feedback.as_ref().unwrap().message,
        "Log level changed to DEBUG"
    );
    assert_eq!(app.feedback.as_ref().unwrap().severity, Severity::Info);
    let changed = app.feedback.as_ref().unwrap().started;
    app.update(Event::Log(record));
    assert_eq!(app.feedback.as_ref().unwrap().started, changed);
    assert_eq!(app.logs.unseen, 3);
}

#[test]
fn command_feedback_is_human_readable_and_repeated_events_do_not_restart_fade() {
    let mut app = app();
    let mut command: Command = serde_json::from_value(json!({
        "schema_version":1, "command_id":"88767477-2a2a-481f-843b-30d56a5e3f10",
        "status":"confirmed","output":"light","requested_enabled":true,
        "reason_code":null,
    }))
    .unwrap();
    app.update(Event::Command(command.clone()));
    assert_eq!(app.feedback.as_ref().unwrap().message, "Light ON confirmed");
    assert_eq!(app.feedback.as_ref().unwrap().severity, Severity::Success);
    let started = app.feedback.as_ref().unwrap().started;
    app.update(Event::Command(command.clone()));
    assert_eq!(app.feedback.as_ref().unwrap().started, started);
    command.status = "rejected".into();
    command.reason_code = Some("telemetry_unavailable".into());
    app.update(Event::Command(command));
    assert_eq!(
        app.feedback.as_ref().unwrap().message,
        "Light ON failed: telemetry unavailable"
    );
    let screen = text(&render(&mut app, 94, 29));
    assert!(!screen.contains("88767477"));
    assert!(!screen.contains("telemetry_unavailable"));
    assert!(screen.contains("Light ON failed: telemetry unavailable"));
}

#[test]
fn dashboard_and_log_modal_stop_growing_at_their_defined_sizes() {
    let mut app = app();
    let dashboard = render(&mut app, 120, 40);
    let x = (120 - ui::DASHBOARD_WIDTH) / 2;
    let y = (40u16 - (ui::DASHBOARD_HEIGHT + 1)).div_ceil(2);
    assert_eq!(dashboard[(x, y)].symbol(), "╭");
    assert_eq!(
        dashboard[(x + ui::DASHBOARD_WIDTH - 1, y + ui::DASHBOARD_HEIGHT - 1)].symbol(),
        "╯"
    );
    app.view = View::Logs;
    let overlay = render(&mut app, 120, 40);
    assert_eq!(overlay[(x + 2, y + 2)].symbol(), "╭");
    assert_eq!(
        overlay[(x + ui::DASHBOARD_WIDTH - 3, y + ui::DASHBOARD_HEIGHT - 3)].symbol(),
        "╯"
    );
    let larger = render(&mut app, 160, 60);
    assert_eq!(larger[(33, 16)].symbol(), "╭");
    assert_eq!(larger[(35, 18)].symbol(), "╭");
}

#[test]
fn log_modal_dims_dashboard_without_stopping_status_updates() {
    let mut app = app();
    let normal = render(&mut app, 94, 24);
    let title = app.title;
    app.view = View::Logs;
    let overlay = render(&mut app, 94, 24);
    assert_eq!(
        normal[(title.x, title.y)].symbol(),
        overlay[(title.x, title.y)].symbol()
    );
    assert_ne!(
        normal[(title.x, title.y)].fg,
        overlay[(title.x, title.y)].fg
    );
    assert!(app.controls.iter().all(|rect| rect.is_empty()));
    let mut status = status();
    status.telemetry.sample.as_mut().unwrap().battery_percent = 81;
    app.update(Event::Status(Box::new(status)));
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(text(&render(&mut app, 94, 24)).contains("81%"));
}

#[test]
fn log_scrollbar_tracks_overflow_scroll_filter_and_resize() {
    let mut app = app();
    app.view = View::Logs;
    app.logs.follow = true;
    for sequence in 0..80 {
        app.update(Event::Log(json!({
            "sequence": sequence,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
            "level": "INFO",
            "message": format!("Record {sequence:03}"),
        })));
    }
    let bottom = render(&mut app, 60, 19);
    let thumb_rows = |buffer: &Buffer| {
        (0..buffer.area.height)
            .filter(|&y| (0..buffer.area.width).any(|x| buffer[(x, y)].symbol() == "█"))
            .collect::<Vec<_>>()
    };
    assert!(!thumb_rows(&bottom).is_empty());
    assert!(text(&bottom).contains("Record 079"));
    app.mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 30,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });
    assert!(
        !text(&render(&mut app, 60, 19))
            .lines()
            .any(|line| line.starts_with("│") && line.contains("Record 079"))
    );
    app.logs.offset = 0;
    let top = render(&mut app, 60, 19);
    assert!(text(&top).contains("Record 000"));
    assert!(
        !text(&top)
            .lines()
            .any(|line| line.starts_with("│") && line.contains("Record 079"))
    );
    assert!(thumb_rows(&top)[0] < thumb_rows(&bottom)[0]);
    app.logs.follow = true;
    assert!(text(&render(&mut app, 60, 19)).contains("Record 079"));
    app.logs.follow = false;
    app.logs.offset = 1000;
    render(&mut app, 120, 40);
    assert_eq!(app.logs.offset, 80 - app.logs.viewport);
    app.logs.records.truncate(4);
    assert!(thumb_rows(&render(&mut app, 60, 19)).is_empty());
}

#[test]
fn escape_closes_help_and_logs_instead_of_quitting() {
    let mut app = app();
    app.key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert!(app.view == View::Help);
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Effect::None
    ));
    assert!(app.view == View::Dashboard);
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE)),
        Effect::Logs(_)
    ));
    app.key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.view == View::Dashboard);
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.view == View::Dashboard);
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Effect::None
    ));
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        Effect::None
    ));
    assert!(app.view == View::Quit);
    let screen = text(&render(&mut app, 80, 24));
    assert!(screen.contains("Quit MyPowers?"));
    assert!(app.controls.iter().all(|rect| rect.is_empty()));
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.view == View::Dashboard);
    app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Effect::Quit
    ));
    app.key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        Effect::Quit
    ));
}

#[test]
fn logs_title_double_click_copies_loaded_rows_without_viewport_or_message_clipping() {
    let mut app = app();
    app.view = View::Logs;
    app.logs = crate::logs::Logs::new(Some(chrono_tz::Europe::Warsaw));
    let long_message = format!("{} END", "x".repeat(3500));
    for sequence in 0..80 {
        app.logs.records.push_back(json!({
            "timestamp":"2026-10-05T10:00:00Z", "level":"INFO",
            "message":if sequence == 79 { long_message.clone() } else { format!("Record {sequence:03}") },
        }));
    }
    app.logs.offset = 20;
    let screen = text(&render(&mut app, 94, 28));
    assert!(!screen.contains("Record 000") && !screen.contains(" END"));
    let title = app.logs.title;
    assert_eq!(
        screen
            .lines()
            .nth(title.y as usize)
            .unwrap()
            .chars()
            .skip(title.x as usize)
            .take(4)
            .collect::<String>(),
        "LOGS"
    );
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: title.x + 1,
        row: title.y,
        modifiers: KeyModifiers::NONE,
    };
    assert!(matches!(app.mouse(click), Effect::None));
    render(&mut app, 94, 28);
    assert!(matches!(app.mouse(click), Effect::CopyLogs));
    let copied = app.logs.clipboard_text();
    assert_eq!(copied.lines().count(), 80);
    assert!(copied.starts_with("2026-10-05T12:00:00+02:00 INFO Record 000\n"));
    assert!(copied.ends_with(&format!("{long_message}\n")));
    assert_eq!(app.logs.offset, 20);
    app.update(Event::Copied(true, ClipboardTarget::Logs));
    let screen = text(&render(&mut app, 94, 28));
    assert!(screen.contains("Logs copied") && !screen.contains("JSON copied"));
    assert!(matches!(app.mouse(click), Effect::None));
    app.resize();
    render(&mut app, 94, 28);
    assert!(matches!(app.mouse(click), Effect::None));
    render(&mut app, 50, 15);
    assert!(app.logs.title.is_empty());
    assert!(matches!(app.mouse(click), Effect::None));
}

#[test]
fn archived_logs_stay_still_during_live_arrivals_and_scrollbar_drag() {
    let mut app = app();
    app.view = View::Logs;
    app.logs.follow = true;
    let stamp = chrono::Utc::now();
    let record = |sequence| {
        json!({
            "sequence": sequence, "timestamp": (stamp + chrono::Duration::milliseconds(sequence)).to_rfc3339(),
            "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10", "level":"INFO",
            "message":format!("Record {sequence:03}"),
        })
    };
    for sequence in 0..80 {
        app.update(Event::Log(record(sequence)));
    }
    render(&mut app, 94, 24);
    let thumb = app.logs.thumb;
    assert!(!thumb.is_empty());
    let mouse = |kind, row| MouseEvent {
        kind,
        column: thumb.x,
        row,
        modifiers: KeyModifiers::NONE,
    };
    app.mouse(mouse(MouseEventKind::Down(MouseButton::Left), thumb.y));
    app.mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        app.logs.scrollbar.y + 3,
    ));
    let offset = app.logs.offset;
    assert!(offset > 0 && offset < app.logs.max_offset());
    let before = text(&render(&mut app, 94, 24));
    for sequence in 80..100 {
        app.update(Event::Log(record(sequence)));
    }
    let after = text(&render(&mut app, 94, 24));
    assert_eq!(app.logs.offset, offset);
    assert_eq!(app.logs.records.len(), 80);
    assert_eq!(app.logs.unseen, 20);
    let content = |screen: &str| {
        screen
            .lines()
            .filter(|line| line.starts_with("│") && line.contains("Record"))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(content(&before), content(&after));
    app.mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        app.logs.scrollbar.y + 3,
    ));
    assert!(!app.logs.follow);
    app.resize();
    let held = app.logs.offset;
    app.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 0));
    assert_eq!(app.logs.offset, held);
    let Effect::Logs(request) = app.key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)) else {
        panic!("Expected day query");
    };
    let page: crate::logs::Page = serde_json::from_value(json!({
        "schema_version":1, "items":[record(95),record(96),record(97),record(98),record(99)],
        "previous_cursor":"before", "next_cursor":"after", "has_more_before":true,
        "has_more_after":false, "source":"files", "gap":false,"skipped_lines":0,
    }))
    .unwrap();
    app.update(Event::LogPage(request, Ok(page)));
    let screen = text(&render(&mut app, 94, 24));
    assert!(screen.contains("Record 099") && app.logs.follow);
}

#[test]
fn archived_records_replayed_by_the_stream_do_not_count_as_unseen() {
    let mut logs = crate::logs::Logs::new(Some(chrono_tz::UTC));
    logs.layout(2);
    let stamp = chrono::Utc::now().to_rfc3339();
    let record = |sequence| {
        json!({
            "sequence": sequence, "timestamp": stamp,
            "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
            "level": "INFO", "message": format!("Record {sequence}")
        })
    };
    let request = logs.open().unwrap();
    let page = serde_json::from_value(json!({
        "schema_version": 1, "items": [record(1), record(2), record(3), record(4)],
        "previous_cursor": null, "next_cursor": null,
        "has_more_before": false, "has_more_after": false,
        "source": "files", "gap": false, "skipped_lines": 0
    }))
    .unwrap();
    logs.accept(&request, Ok(page));
    logs.offset = 1;
    let archive = logs.clipboard_text();
    for sequence in 1..=4 {
        assert!(logs.record(record(sequence)));
    }
    assert_eq!(logs.unseen, 0);
    assert_eq!(logs.offset, 1);
    assert_eq!(logs.clipboard_text(), archive);
    assert!(logs.record(record(5)));
    assert_eq!(logs.unseen, 1);
    assert!(!logs.record(record(5)));
    assert_eq!(logs.unseen, 1);
    // Sequences belong to a daemon instance; another instance's same sequence is new.
    let mut other_instance = record(1);
    other_instance["server_instance_id"] = json!("d1b293be-fc79-40b6-9f6b-ae38b507e36c");
    assert!(logs.record(other_instance));
    assert_eq!(logs.unseen, 2);
    assert_eq!(logs.offset, 1);
    assert_eq!(logs.clipboard_text(), archive);
}

#[test]
fn log_pages_ignore_old_responses_and_keep_cursor_edges_when_cache_is_bounded() {
    let mut logs = crate::logs::Logs::new(Some(chrono_tz::UTC));
    logs.page_size = 50;
    logs.layout(10);
    let first = logs.open().unwrap();
    let current = logs.navigate(false).unwrap();
    let page = |start: usize| {
        serde_json::from_value::<crate::logs::Page>(json!({
        "schema_version":1, "items": (start..start+50).map(|sequence| json!({
            "sequence":sequence,"message":format!("Record {sequence}"),"level":"INFO",
            "timestamp":"2020-01-01T12:00:00Z", "server_instance_id":"88767477-2a2a-481f-843b-30d56a5e3f10"
        })).collect::<Vec<_>>(),
        "previous_cursor":format!("before-{start}"), "next_cursor":format!("after-{}",start+49),
        "has_more_before":true, "has_more_after":false, "source":"files", "gap":false,"skipped_lines":0,
    })).unwrap()
    };
    logs.accept(&first, Ok(page(400)));
    assert!(logs.records.is_empty() && logs.loading);
    logs.accept(&current, Ok(page(350)));
    for start in (0..350).step_by(50).rev() {
        logs.offset = 0;
        let request = logs.scroll(false, 1).unwrap();
        assert_eq!(request.kind, crate::logs::Load::Older);
        assert_eq!(request.limit, 50);
        logs.accept(&request, Ok(page(start)));
        assert!(logs.records.len() <= 250);
    }
    assert_eq!(logs.records.front().unwrap()["sequence"], 0);
    assert_eq!(logs.records.back().unwrap()["sequence"], 249);
    logs.offset = logs.max_offset();
    let newer = logs.scroll(true, 1).unwrap();
    assert_eq!(newer.kind, crate::logs::Load::Newer);
    assert_eq!(newer.cursor.as_deref(), Some("after-249"));
    logs.accept(&newer, Ok(page(250)));
    assert_eq!(logs.records.len(), 250);
    assert_eq!(logs.records.front().unwrap()["sequence"], 50);
    logs.offset = 0;
    let older = logs.scroll(false, 1).unwrap();
    assert_eq!(older.cursor.as_deref(), Some("before-50"));
}

#[test]
fn new_log_ranges_discard_previous_pagination_even_when_their_request_fails() {
    use crate::logs::{Load, Logs, Page};

    for transition in ["filter", "previous day", "today", "midnight"] {
        let mut logs = Logs::new(Some(chrono_tz::UTC));
        logs.layout(1);
        if matches!(transition, "today" | "midnight") {
            logs.day = logs.day.pred_opt().unwrap();
        }
        let initial = logs.open().unwrap();
        let stamp = logs.day.format("%Y-%m-%dT12:00:00Z").to_string();
        let page: Page = serde_json::from_value(json!({
            "schema_version": 1, "items": [
                {"sequence": 1, "message": "First archived record", "level": "INFO",
                 "timestamp": stamp, "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10"},
                {"sequence": 2, "message": "Second archived record", "level": "INFO",
                 "timestamp": stamp, "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10"}
            ],
            "previous_cursor": "previous-range-before", "next_cursor": "previous-range-after",
            "has_more_before": true, "has_more_after": true,
            "source": "files", "gap": false, "skipped_lines": 0
        }))
        .unwrap();
        logs.accept(&initial, Ok(page));
        logs.offset = 1;
        let request = match transition {
            "filter" => logs.key(KeyCode::Char('f')),
            "previous day" => logs.navigate(false),
            "today" => logs.key(KeyCode::End),
            "midnight" => {
                logs.follow = true;
                logs.maintenance()
            }
            _ => unreachable!(),
        }
        .unwrap();
        assert!(request.cursor.is_none(), "{transition}");
        logs.accept(&request, Err("HTTP temporarily unavailable".into()));
        assert!(logs.records.is_empty(), "{transition}");
        assert!(!logs.more_before && !logs.more_after, "{transition}");
        assert_eq!(logs.offset, 0, "{transition}");
        let older_day = logs.scroll(false, 1).unwrap();
        assert_eq!(older_day.kind, Load::Latest, "{transition}");
        assert!(older_day.cursor.is_none(), "{transition}");
        assert_ne!(older_day.since, request.since, "{transition}");
    }
}

#[test]
fn failed_log_refresh_within_the_same_range_preserves_loaded_pagination() {
    use crate::logs::{Load, Logs, Page};

    for key in [KeyCode::Home, KeyCode::End, KeyCode::Char('+')] {
        let mut logs = Logs::new(Some(chrono_tz::UTC));
        let initial = logs.open().unwrap();
        let stamp = logs.day.format("%Y-%m-%dT12:00:00Z").to_string();
        let page: Page = serde_json::from_value(json!({
            "schema_version": 1, "items": [{
                "sequence": 1, "message": "Archived record", "level": "INFO", "timestamp": stamp,
                "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10"
            }],
            "previous_cursor": "same-range-before", "next_cursor": "same-range-after",
            "has_more_before": true, "has_more_after": true,
            "source": "files", "gap": false, "skipped_lines": 0
        }))
        .unwrap();
        logs.accept(&initial, Ok(page));
        let request = logs.key(key).unwrap();
        logs.accept(&request, Err("HTTP temporarily unavailable".into()));
        assert_eq!(logs.records.len(), 1);
        assert!(logs.more_before && logs.more_after);
        let older = logs.scroll(false, 1).unwrap();
        assert_eq!(older.kind, Load::Older);
        assert_eq!(older.cursor.as_deref(), Some("same-range-before"));
    }
}

#[test]
fn failed_automatic_log_refresh_waits_before_retrying_but_end_remains_immediate() {
    let mut logs = crate::logs::Logs::new(Some(chrono_tz::UTC));
    logs.page_size = 50;
    let initial = logs.key(KeyCode::End).unwrap();
    let stamp = chrono::Utc::now().to_rfc3339();
    let page = serde_json::from_value(json!({
        "schema_version": 1, "items": [], "previous_cursor": null, "next_cursor": null,
        "has_more_before": false, "has_more_after": false,
        "source": "files", "gap": false, "skipped_lines": 0
    }))
    .unwrap();
    logs.accept(&initial, Ok(page));
    for sequence in 0..251 {
        logs.record(json!({
            "sequence": sequence, "timestamp": stamp,
            "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
            "level": "DEBUG", "message": "Live record"
        }));
    }
    let automatic = logs.maintenance().unwrap();
    logs.accept(&automatic, Err("HTTP temporarily unavailable".into()));
    for _ in 0..1000 {
        assert!(
            logs.maintenance().is_none(),
            "Failed refresh retried in a busy loop"
        );
    }
    let manual = logs
        .key(KeyCode::End)
        .expect("End must bypass automatic retry delay");
    assert!(manual.generation > automatic.generation);
    assert_eq!(manual.kind, crate::logs::Load::Latest);
}

#[test]
fn live_log_cache_stays_bounded_when_page_refresh_fails_and_recovers_from_the_server() {
    let stamp = chrono::Utc::now().to_rfc3339();
    let mut logs = crate::logs::Logs::new(Some(chrono_tz::UTC));
    logs.page_size = 50;
    logs.layout(10);
    let record = |sequence| {
        json!({
            "sequence": sequence, "timestamp": stamp,
            "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
            "level": "INFO", "message": format!("Record {sequence}")
        })
    };
    let page = |start| {
        serde_json::from_value::<crate::logs::Page>(json!({
            "schema_version": 1,
            "items": (start..start + 50).map(record).collect::<Vec<_>>(),
            "previous_cursor": format!("before-{start}"), "next_cursor": null,
            "has_more_before": true, "has_more_after": false,
            "source": "files", "gap": false, "skipped_lines": 0
        }))
        .unwrap()
    };
    let initial = logs.key(KeyCode::End).unwrap();
    logs.accept(&initial, Ok(page(0)));
    for batch in 0..15 {
        for sequence in 50 + batch * 200..50 + (batch + 1) * 200 {
            assert!(logs.record(record(sequence)));
        }
        if let Some(refresh) = logs.maintenance() {
            assert!(logs.loading);
            logs.accept(&refresh, Err("HTTP temporarily unavailable".into()));
        }
        // Five loaded pages plus the existing 1,000-record live catch-up allowance.
        assert!(
            logs.records.len() <= 1250,
            "Unbounded cache after batch {batch}"
        );
        assert!(logs.follow && !logs.loading);
        assert_eq!(logs.offset, logs.max_offset());
    }
    let retained = logs.clipboard_text();
    let retry = logs.key(KeyCode::End).unwrap();
    logs.accept(&retry, Err("HTTP temporarily unavailable".into()));
    assert_eq!(logs.clipboard_text(), retained);
    let recovered = logs.key(KeyCode::End).unwrap();
    assert!(logs.record(record(3050)));
    logs.accept(&recovered, Ok(page(3000)));
    assert_eq!(logs.records.len(), 51);
    assert_eq!(logs.records.front().unwrap()["sequence"], 3000);
    assert_eq!(logs.records.back().unwrap()["sequence"], 3050);
    assert!(logs.follow);
    assert_eq!(logs.offset, logs.max_offset());
    assert_eq!(logs.unseen, 0, "Caught-up live rows are already visible");
    assert!(logs.record(record(3051)));
    assert_eq!(logs.records.back().unwrap()["sequence"], 3051);
    logs.offset = 0;
    let older = logs.scroll(false, 1).unwrap();
    assert_eq!(older.cursor.as_deref(), Some("before-3000"));
}

#[test]
fn live_and_archived_log_filters_preserve_critical_records() {
    let now = chrono::Utc::now();
    let record = |sequence, level| {
        json!({
            "sequence": sequence, "timestamp": now.to_rfc3339(),
            "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
            "level": level, "message": format!("Message {level}")
        })
    };
    for minimum in 0..crate::logs::LEVELS.len() {
        let mut logs = crate::logs::Logs::new_at(Some(chrono_tz::UTC), now);
        logs.level = minimum;
        logs.follow = true;
        for (sequence, level) in ["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"]
            .into_iter()
            .enumerate()
        {
            assert!(logs.record(record(sequence, level)));
        }
        let levels: Vec<_> = logs
            .records
            .iter()
            .map(|r| r["level"].as_str().unwrap())
            .collect();
        assert_eq!(
            levels,
            ["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"][minimum..]
        );
        assert!(logs.clipboard_text().contains("CRITICAL Message CRITICAL"));
        let critical = record(5, "CRITICAL");
        logs.follow = false;
        assert!(logs.record(critical.clone()));
        assert_eq!(logs.unseen, 1);
        assert!(!logs.record(critical.clone()));
        assert_eq!(logs.unseen, 1);
        let feedback = Feedback::log(&critical).expect("Critical errors need operational feedback");
        assert_eq!(feedback.message, "Message CRITICAL");
        assert_eq!(feedback.severity, Severity::Error);
        assert!(Feedback::log(&record(6, "DEBUG")).is_none());
        let request = logs.key(KeyCode::End).unwrap();
        logs.accept(
            &request,
            Ok(serde_json::from_value(json!({
                "schema_version": 1, "items": [], "previous_cursor": null,
                "next_cursor": null, "has_more_before": false, "has_more_after": false,
                "source": "files", "gap": false, "skipped_lines": 0
            }))
            .unwrap()),
        );
        assert_eq!(logs.records.back().unwrap(), &critical);
        assert_eq!(logs.unseen, 0);
    }
}

#[test]
fn invalid_log_day_bounds_discard_pending_responses_and_allow_navigation_to_recover() {
    use crate::clock::Clock;
    let now = "2011-12-30T12:00:00Z".parse().unwrap();
    for (succeeds, days_back) in [(false, 1), (true, 1), (false, 2), (true, 2)] {
        let mut app = App::with_clock(
            false,
            Some(chrono_tz::Pacific::Apia),
            Clock::Fixed {
                now,
                telemetry_elapsed: Duration::ZERO,
                animation_elapsed: Duration::ZERO,
                feedback_elapsed: Duration::ZERO,
            },
        );
        app.view = View::Logs;
        let pending = app.logs.open().unwrap();
        for _ in 0..days_back {
            assert!(app.logs.navigate(false).is_none());
        }
        assert_eq!(
            app.logs.day.to_string(),
            if days_back == 1 {
                "2011-12-30"
            } else {
                "2011-12-29"
            }
        );
        let error = app.logs.message.clone();
        assert!(error.contains(if days_back == 1 {
            "valid local midnight"
        } else {
            "next day boundary"
        }));
        assert!(!app.logs.loading);
        assert_ne!(app.logs.generation(), pending.generation);
        app.feedback = None;
        let page = |sequence| {
            serde_json::from_value(json!({
                "schema_version": 1, "items": [{
                    "sequence": sequence, "timestamp": "2011-12-30T12:00:00Z",
                    "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
                    "level": "INFO", "message": format!("Record {sequence}")
                }], "previous_cursor": null, "next_cursor": null,
                "has_more_before": false, "has_more_after": false,
                "source": "files", "gap": false, "skipped_lines": 0
            }))
            .unwrap()
        };
        app.update(Event::LogPage(
            pending,
            if succeeds {
                Ok(page(1))
            } else {
                Err("Old request failed".into())
            },
        ));
        assert!(app.logs.records.is_empty());
        assert_eq!(app.logs.message, error);
        assert!(!app.logs.loading);
        assert!(
            app.feedback.is_none(),
            "An obsolete response must not replace status feedback"
        );
        for _ in 1..days_back {
            assert!(app.logs.navigate(true).is_none());
        }
        let recovered = app.logs.navigate(true).unwrap();
        app.update(Event::LogPage(recovered, Ok(page(2))));
        assert_eq!(app.logs.day.to_string(), "2011-12-31");
        assert_eq!(app.logs.records.len(), 1);
        assert_eq!(app.logs.records[0]["sequence"], 2);
        assert!(!app.logs.loading);
    }
}

#[test]
fn log_day_bounds_use_iana_dst_transitions_and_midnight_offsets() {
    let zone = Some(chrono_tz::Europe::Warsaw);
    for (date, hours) in [("2026-03-29", 23), ("2026-10-25", 25), ("2026-10-05", 24)] {
        let (start, end) = crate::logs::day_bounds(date.parse().unwrap(), zone).unwrap();
        let start = chrono::DateTime::parse_from_rfc3339(&start).unwrap();
        let end = chrono::DateTime::parse_from_rfc3339(&end).unwrap();
        assert_eq!((end - start).num_hours(), hours);
        assert_eq!(
            start
                .with_timezone(&chrono_tz::Europe::Warsaw)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            format!("{date} 00:00")
        );
    }
    let (start, _) = crate::logs::day_bounds("2026-10-05".parse().unwrap(), zone).unwrap();
    assert!(start.starts_with("2026-10-04T22:00:00"));
    for (zone, date, first_time, expected_start, expected_end) in [
        (
            chrono_tz::America::Havana,
            "2026-11-01",
            "00:00",
            "2026-11-01T04:00:00Z",
            "2026-11-02T05:00:00Z",
        ),
        (
            chrono_tz::America::Sao_Paulo,
            "2018-11-04",
            "01:00",
            "2018-11-04T03:00:00Z",
            "2018-11-05T02:00:00Z",
        ),
        (
            chrono_tz::Asia::Kathmandu,
            "1986-01-01",
            "00:15",
            "1985-12-31T18:30:00Z",
            "1986-01-01T18:15:00Z",
        ),
    ] {
        let (start, end) = crate::logs::day_bounds(date.parse().unwrap(), Some(zone)).unwrap();
        let start = chrono::DateTime::parse_from_rfc3339(&start).unwrap();
        let end = chrono::DateTime::parse_from_rfc3339(&end).unwrap();
        assert_eq!(
            start,
            chrono::DateTime::parse_from_rfc3339(expected_start).unwrap()
        );
        assert_eq!(
            end,
            chrono::DateTime::parse_from_rfc3339(expected_end).unwrap()
        );
        assert_eq!(
            start
                .with_timezone(&zone)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            format!("{date} {first_time}")
        );
    }
}

#[test]
fn click_activates_once_on_release_and_resize_or_revision_discards_old_press() {
    let mut app = app();
    render(&mut app, 80, 24);
    let rect = app.controls[0];
    let event = |kind| MouseEvent {
        kind,
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    assert!(matches!(
        app.mouse(event(MouseEventKind::Down(MouseButton::Left))),
        Effect::None
    ));
    assert!(matches!(
        app.mouse(event(MouseEventKind::Up(MouseButton::Left))),
        Effect::Request(_)
    ));
    assert!(matches!(
        app.mouse(event(MouseEventKind::Up(MouseButton::Left))),
        Effect::None
    ));
    app.update(Event::Finished(Feedback::new(
        "AC ON confirmed",
        Severity::Success,
    )));
    app.mouse(event(MouseEventKind::Down(MouseButton::Left)));
    app.resize();
    render(&mut app, 80, 24);
    assert!(matches!(
        app.mouse(event(MouseEventKind::Up(MouseButton::Left))),
        Effect::None
    ));
    app.mouse(event(MouseEventKind::Down(MouseButton::Left)));
    let mut changed = status();
    changed.controls.outputs_revision += 1;
    app.update(Event::Status(Box::new(changed)));
    assert!(matches!(
        app.mouse(event(MouseEventKind::Up(MouseButton::Left))),
        Effect::None
    ));
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Effect::None
    ));
}

#[test]
fn aggregate_history_keeps_numeric_telemetry_independent_and_refreshes_are_bounded() {
    let mut app = fixed_graph_app();
    let request = app.history_request().unwrap();
    assert_eq!(request.resolution, Resolution::TenSeconds);
    assert_eq!(request.width, 43);
    assert_eq!(
        request.since.timestamp_millis(),
        app.timeline_now_ms() - 420_000
    );
    assert!(
        app.history_request().is_none(),
        "Only one query may be in flight"
    );
    let before = serde_json::to_value(app.status.as_ref().unwrap()).unwrap();
    app.update(Event::History(
        request.clone(),
        Ok(vec![Point {
            bucket_start_ms: request.since.timestamp_millis(),
            input_power_w: 0.5,
            output_power_w: 1.5,
            sample_count: 2,
        }]),
    ));
    assert_eq!(app.graph.points.len(), 1);
    assert_eq!(app.graph_data(43, false)[0], 0.5);
    assert_eq!(
        serde_json::to_value(app.status.as_ref().unwrap()).unwrap(),
        before
    );
    assert!(app.allowed());
    assert!(
        app.history_request().is_none(),
        "Do not query on every telemetry event"
    );
    app.update(Event::Disconnected("Temporary disconnect".into()));
    app.update(Event::Status(Box::new(
        app.status.as_ref().unwrap().clone(),
    )));
    let recovered = app.history_request().unwrap();
    assert!(recovered.generation > request.generation);
    app.update(Event::History(request, Ok(vec![])));
    assert_eq!(app.graph.points.len(), 1);
    app.update(Event::History(recovered, Err("HTTP unavailable".into())));
    assert!(
        app.history_request().is_none(),
        "Failed reads must wait before retrying"
    );
    assert_eq!(
        app.graph.points.len(),
        1,
        "Keep already loaded history on failure"
    );
    assert!(app.allowed());
}

#[test]
fn time_shortcut_is_contextual_and_discards_obsolete_resolution_and_resize_responses() {
    let mut app = fixed_graph_app();
    let first = app.history_request().unwrap();
    let before = serde_json::to_value(app.status.as_ref().unwrap()).unwrap();
    for (label, resolution) in [
        ("30s", Resolution::ThirtySeconds),
        ("60s", Resolution::Minute),
        ("1h", Resolution::Hour),
        ("10s", Resolution::TenSeconds),
    ] {
        app.key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(app.graph.resolution, resolution);
        let request = app.history_request().unwrap();
        assert_eq!(request.resolution, resolution);
        assert_eq!(
            request.until.timestamp_millis() - request.since.timestamp_millis(),
            42 * resolution.seconds() * 1000 + 1
        );
        assert!(text(&render(&mut app, 94, 29)).contains(&format!("t {label}")));
        app.update(Event::History(
            first.clone(),
            Ok(vec![Point {
                bucket_start_ms: first.since.timestamp_millis(),
                input_power_w: 99.0,
                output_power_w: 299.0,
                sample_count: 1,
            }]),
        ));
        assert!(app.graph.points.is_empty());
    }
    assert_eq!(
        serde_json::to_value(app.status.as_ref().unwrap()).unwrap(),
        before
    );
    for view in [View::Logs, View::Settings, View::Help, View::Quit] {
        app.view = view;
        app.key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(app.graph.resolution, Resolution::TenSeconds);
    }
    app.view = View::Dashboard;
    app.key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert_eq!(app.graph.resolution, Resolution::TenSeconds);
    app.set_graph_width(27);
    let resized = app.history_request().unwrap();
    assert_eq!(resized.width, 27);
    assert!(resized.generation > first.generation);
    app.set_graph_width(27);
    assert!(
        app.history_request().is_none(),
        "An unchanged width must not invalidate the request"
    );
}

#[test]
fn completed_aggregate_bars_keep_values_and_colors_until_a_whole_column_shift() {
    use crate::clock::Clock;
    let mut app = fixed_graph_app();
    let now_ms = app.timeline_now_ms();
    let now = app.clock.now();
    app.selected = None;
    app.feedback = None;
    for resolution in [Resolution::TenSeconds, Resolution::Minute, Resolution::Hour] {
        app.graph.resolution = resolution;
        let span = resolution.seconds() * 1000;
        app.graph.points = (0..44)
            .map(|index| {
                let low = index % 2 == 0;
                Point {
                    bucket_start_ms: now_ms - (43 - index) * span,
                    input_power_w: if low { 44.0 } else { 47.0 },
                    output_power_w: if low { 134.0 } else { 141.0 },
                    sample_count: 6,
                }
            })
            .collect();
        for width in [27, 40, 44] {
            for output in [false, true] {
                app.clock = Clock::Fixed {
                    now,
                    telemetry_elapsed: Duration::from_millis((span / 10) as u64),
                    animation_elapsed: Duration::ZERO,
                    feedback_elapsed: Duration::ZERO,
                };
                let first = app.graph_data(width, output);
                app.clock = Clock::Fixed {
                    now,
                    telemetry_elapsed: Duration::from_millis((span * 8 / 10) as u64),
                    animation_elapsed: Duration::ZERO,
                    feedback_elapsed: Duration::ZERO,
                };
                assert_eq!(app.graph_data(width, output), first);
                app.clock = Clock::Fixed {
                    now,
                    telemetry_elapsed: Duration::from_millis((span * 11 / 10) as u64),
                    animation_elapsed: Duration::ZERO,
                    feedback_elapsed: Duration::ZERO,
                };
                assert_eq!(
                    app.graph_data(width, output)[..usize::from(width) - 1],
                    first[1..]
                );
            }
        }
        app.clock = Clock::Fixed {
            now,
            telemetry_elapsed: Duration::from_millis(250),
            animation_elapsed: Duration::ZERO,
            feedback_elapsed: Duration::ZERO,
        };
        let first = render(&mut app, 94, 29);
        app.clock = Clock::Fixed {
            now,
            telemetry_elapsed: Duration::from_millis(1000),
            animation_elapsed: Duration::ZERO,
            feedback_elapsed: Duration::ZERO,
        };
        assert_eq!(render(&mut app, 94, 29), first);
    }
}

#[test]
fn aggregate_graph_preserves_missing_buckets_measured_zeros_and_window_expiry() {
    use crate::clock::Clock;
    let mut app = fixed_graph_app();
    let now_ms = app.timeline_now_ms();
    app.set_graph_width(40);
    app.graph.points = vec![
        Point {
            bucket_start_ms: now_ms - 390_000,
            input_power_w: 0.5,
            output_power_w: 1.5,
            sample_count: 2,
        },
        Point {
            bucket_start_ms: now_ms - 20_000,
            input_power_w: 0.0,
            output_power_w: 0.0,
            sample_count: 1,
        },
    ];
    let data = app.graph_data(40, false);
    assert_eq!(data[0], 0.5);
    assert!(data[1..].iter().all(|&value| value == 0.0));
    assert_eq!(
        app.graph.points[1].sample_count, 1,
        "A measured zero remains a real bucket"
    );
    assert_eq!(app.graph_data(0, false), Vec::<f64>::new());
    assert!(app.has_power_history(false) && app.has_power_history(true));
    app.status
        .as_mut()
        .unwrap()
        .telemetry
        .sample
        .as_mut()
        .unwrap()
        .input_power_w = 0;
    app.status
        .as_mut()
        .unwrap()
        .telemetry
        .sample
        .as_mut()
        .unwrap()
        .output_power_w = 0;
    app.clock = Clock::Fixed {
        now: app.clock.now(),
        telemetry_elapsed: Duration::from_secs(10),
        animation_elapsed: Duration::ZERO,
        feedback_elapsed: Duration::ZERO,
    };
    assert!(!app.has_power_history(false) && !app.has_power_history(true));
    app.prune_graph();
    assert_eq!(app.graph.points.len(), 1);
}

#[test]
fn daemon_restart_and_backwards_server_clock_discard_cached_aggregates() {
    let mut app = fixed_graph_app();
    seed_current_graph(&mut app);
    let old = app.history_request().unwrap();
    let mut restarted = app.status.as_ref().unwrap().clone();
    restarted.server_instance_id = "4621ca79-497e-411c-a959-080c7527c01a".into();
    app.update(Event::Status(Box::new(restarted.clone())));
    assert!(app.graph.points.is_empty());
    app.update(Event::History(old, Ok(vec![])));
    let fresh = app.history_request().unwrap();
    assert_eq!(fresh.server_instance_id, restarted.server_instance_id);
    seed_current_graph(&mut app);
    restarted.server_time = (app.clock.now() - chrono::Duration::seconds(1)).to_rfc3339();
    app.update(Event::Status(Box::new(restarted)));
    assert!(app.graph.points.is_empty());
    assert!(app.history_request().unwrap().generation > fresh.generation);
}

#[test]
fn stream_errors_replace_transient_feedback() {
    let mut app = app();
    app.update(Event::Finished(Feedback::new(
        "Command outcome uncertain; check station",
        Severity::Warning,
    )));
    app.update(Event::Disconnected("Disconnected".into()));
    app.update(Event::Notice("Logs unavailable".into()));
    assert!(
        app.feedback
            .as_ref()
            .unwrap()
            .message
            .contains("Connection lost")
    );
    assert!(!app.connected);
}

#[test]
fn recovered_log_stream_rearms_notices_without_replacing_user_feedback() {
    let mut app = app();
    let notice = "Logs: Daemon stream closed. Reconnecting...";
    app.update(Event::Notice(notice.into()));
    assert_eq!(
        app.feedback.as_ref().unwrap().message,
        "Log stream unavailable; reconnecting"
    );
    app.update(Event::Finished(Feedback::new(
        "AC ON confirmed",
        Severity::Success,
    )));
    let started = app.feedback.as_ref().unwrap().started;
    app.update(Event::Notice(notice.into()));
    assert_eq!(app.feedback.as_ref().unwrap().message, "AC ON confirmed");
    assert_eq!(app.feedback.as_ref().unwrap().started, started);
    app.update(Event::LogStreamReady);
    assert!(app.log_notice.is_empty());
    assert_eq!(app.feedback.as_ref().unwrap().message, "AC ON confirmed");
    assert_eq!(app.feedback.as_ref().unwrap().started, started);
    app.update(Event::Notice(notice.into()));
    assert_eq!(
        app.feedback.as_ref().unwrap().message,
        "Log stream unavailable; reconnecting"
    );
    app.update(Event::Finished(Feedback::new(
        "DC OFF confirmed",
        Severity::Success,
    )));
    let started = app.feedback.as_ref().unwrap().started;
    app.update(Event::Notice(notice.into()));
    assert_eq!(app.feedback.as_ref().unwrap().message, "DC OFF confirmed");
    assert_eq!(app.feedback.as_ref().unwrap().started, started);
}

#[test]
fn runtime_action_feedback_stays_only_in_status_strip_despite_concurrent_log_page() {
    let mut app = app();
    let Effect::Logs(request) = app.key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE)) else {
        panic!("Expected log request");
    };
    app.update(Event::Finished(Feedback::new(
        "Log level changed to DEBUG",
        Severity::Info,
    )));
    let page = serde_json::from_value(json!({
        "schema_version":1,"items":[],"previous_cursor":null,"next_cursor":null,
        "has_more_before":false,"has_more_after":false,"source":"files","gap":false,"skipped_lines":0,
    })).unwrap();
    app.update(Event::LogPage(request, Ok(page)));
    let screen = text(&render(&mut app, 60, 19));
    assert_eq!(screen.matches("Log level changed to DEBUG").count(), 1);
    assert!(
        screen
            .lines()
            .last()
            .unwrap()
            .contains("Log level changed to DEBUG")
    );
    assert!(screen.contains("Logs loaded | 0 loaded"));
    assert!(!screen.contains("Runtime log level updated"));
    app.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    let screen = text(&render(&mut app, 60, 19));
    assert_eq!(screen.matches("Sending request...").count(), 1);
    assert!(
        screen
            .lines()
            .last()
            .unwrap()
            .contains("Sending request...")
    );
    assert!(screen.contains("Logs loaded | 0 loaded"));
}

#[test]
fn shortcuts_are_confined_to_the_active_context() {
    for view in [View::Logs, View::Help, View::Settings, View::Quit] {
        for code in [
            KeyCode::Char('a'),
            KeyCode::Char('d'),
            KeyCode::Char('l'),
            KeyCode::Char('s'),
            KeyCode::F(2),
            KeyCode::F(3),
            KeyCode::F(5),
        ] {
            let mut app = app();
            app.view = view;
            assert!(matches!(
                app.key(KeyEvent::new(code, KeyModifiers::NONE)),
                Effect::None
            ));
            assert!(app.view == view);
            assert!(app.pending.is_none());
        }
    }
    for view in [View::Dashboard, View::Logs, View::Help, View::Quit] {
        for code in [KeyCode::Char('r'), KeyCode::Char('p')] {
            let mut app = app();
            app.view = view;
            assert!(matches!(
                app.key(KeyEvent::new(code, KeyModifiers::NONE)),
                Effect::None
            ));
            assert!(app.pending.is_none());
        }
    }
    for desired in ["running", "paused"] {
        for key in ['r', 'p'] {
            let mut app = app();
            app.view = View::Settings;
            app.settings_tab = SettingsTab::Debug;
            app.status.as_mut().unwrap().connection.desired = desired.into();
            assert!(matches!(
                app.key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
                Effect::None
            ));
            app.settings_selected = if key == 'r' { 0 } else { 1 };
            let effect = app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert!(match (key, effect) {
                ('r', Effect::Request(Intent::Retry)) => true,
                ('p', Effect::Request(Intent::Connection(running))) =>
                    running == (desired == "paused"),
                _ => false,
            });
        }
    }
    for view in [View::Dashboard, View::Help, View::Quit] {
        let mut app = app();
        app.view = view;
        assert!(matches!(
            app.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)),
            Effect::None
        ));
        assert!(app.pending.is_none());
    }
    for view in [
        View::Dashboard,
        View::Logs,
        View::Help,
        View::Settings,
        View::Quit,
    ] {
        let mut app = app();
        app.view = view;
        for code in [KeyCode::Char('c'), KeyCode::Char('z'), KeyCode::Char('a')] {
            assert!(matches!(
                app.key(KeyEvent::new(code, KeyModifiers::CONTROL)),
                Effect::None
            ));
            assert!(app.view == view);
        }
        assert!(matches!(
            app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Effect::Quit
        ));
        app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.view == View::Quit);
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.view == View::Dashboard);
    }
    let mut app = app();
    app.view = View::Logs;
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)),
        Effect::Request(Intent::Debug(true))
    ));
}

#[test]
fn logs_layout_gains_two_rows_and_moves_contextual_diagnostics_to_status() {
    let mut app = app();
    app.view = View::Logs;
    app.feedback = None;
    for sequence in 0..100 {
        app.logs.records.push_back(json!({"timestamp":"2026-10-05T12:00:00Z", "level":"INFO", "message":format!("Record {sequence:03}")}));
    }
    for (width, height, rows) in [(60, 19, 9), (80, 24, 14), (94, 29, 19)] {
        let screen = text(&render(&mut app, width, height));
        assert_eq!(
            screen
                .lines()
                .filter(|line| line.contains("Record"))
                .count(),
            rows
        );
        assert!(screen.contains("Filter ≥ DEBUG"));
        assert!(screen.contains("End live") || screen.contains("End today/live"));
        assert!(screen.contains("Esc close") && screen.contains("q quit"));
        assert!(!screen.contains("refresh") && !screen.contains("F2") && !screen.contains("F3"));
        assert!(!screen.contains("override") && !screen.contains("none"));
        assert!(screen.lines().last().unwrap().ends_with("Log: INFO "));
        assert!(app.title.is_empty());
        assert_eq!(app.logs.buttons.len(), 2);
    }
    app.timezone = Some(chrono_tz::Europe::Warsaw);
    let expiry = chrono::Utc::now() + chrono::Duration::minutes(10);
    app.status.as_mut().unwrap().logging["override_expires_at"] = json!(expiry.to_rfc3339());
    let expected = format!(
        "Log: INFO | until {}",
        expiry
            .with_timezone(&chrono_tz::Europe::Warsaw)
            .format("%Y-%m-%d %H:%M:%S")
    );
    let screen = text(&render(&mut app, 94, 29));
    assert!(screen.lines().last().unwrap().contains(&expected));
    app.status.as_mut().unwrap().logging["override_expires_at"] =
        json!((chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
    let screen = text(&render(&mut app, 94, 29));
    assert!(!screen.contains("until"));
    app.view = View::Dashboard;
    assert!(
        text(&render(&mut app, 94, 29))
            .lines()
            .last()
            .unwrap()
            .trim()
            .is_empty()
    );
}

#[test]
fn help_lists_only_the_context_that_opened_it() {
    for context in [View::Dashboard, View::Logs, View::Settings] {
        let mut app = app();
        app.view = context;
        app.settings_tab = SettingsTab::Debug;
        app.key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.view == View::Help && app.help_context == context);
        let screen = text(&render(&mut app, 94, 29));
        assert!(app.title.is_empty());
        assert!(!screen.contains("Ctrl-Q"));
        if context == View::Logs {
            assert!(screen.contains("HELP — LOGS") && screen.contains("End"));
            assert!(!screen.contains("Request AC") && !screen.contains("F3"));
        } else if context == View::Settings {
            assert!(
                screen.contains("HELP — SETTINGS")
                    && screen.contains("Select Retry / Pause / Debug")
            );
            assert!(screen.contains("Activate selected button") && screen.contains("Click"));
            assert!(!screen.contains("Request AC") && !screen.contains("F3"));
        } else {
            assert!(
                screen.contains("Open Logs") && !screen.contains("Select Retry / Pause / Debug")
            );
            assert!(!screen.contains("Retry station") && !screen.contains("Pause/resume station"));
        }
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.view == View::Dashboard);
    }
}

#[test]
fn positive_power_samples_draw_at_least_one_tick_with_doubling_scales() {
    let symbols = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    for (width, height) in [(60, 19), (80, 24), (120, 30)] {
        for value in [
            0, 1, 3, 6, 7, 18, 19, 20, 99, 100, 101, 299, 300, 301, 65535,
        ] {
            let mut app = app();
            let mut current = status();
            let sample = current.telemetry.sample.as_mut().unwrap();
            sample.sequence = 2;
            sample.input_power_w = value;
            sample.output_power_w = value;
            app.graph.points.clear();
            app.update(Event::Status(Box::new(current)));
            seed_current_graph(&mut app);
            let buffer = render(&mut app, width, height);
            let screen = text(&buffer);
            let power_row = screen
                .lines()
                .position(|line| line.contains("INPUT"))
                .unwrap() as u16;
            let maximum = crate::history::power_scale([value as f64]);
            for (start, end) in [(0, width / 2), (width / 2, width)] {
                let bars: Vec<_> = (start..end)
                    .filter(|&x| {
                        (power_row + 2..=power_row + 3).any(|y| {
                            let symbol = buffer[(x, y)].symbol();
                            symbols[1..].contains(&symbol)
                        })
                    })
                    .collect();
                if value == 0 {
                    assert!(bars.is_empty(), "Zero power must not get a minimum bar");
                    continue;
                }
                assert_eq!(bars.len(), 1, "{width}x{height}: {value} W / {maximum} W");
                let ticks = (value * 16 / maximum).clamp(1, 16) as usize;
                let x = bars[0];
                assert_eq!(buffer[(x, power_row + 3)].symbol(), symbols[ticks.min(8)]);
                assert_eq!(
                    buffer[(x, power_row + 2)].symbol(),
                    symbols[ticks.saturating_sub(8)]
                );
            }
        }
    }
}

#[test]
fn positive_fractional_averages_remain_visible_even_when_live_power_is_zero() {
    let mut app = fixed_graph_app();
    let sample = app
        .status
        .as_mut()
        .unwrap()
        .telemetry
        .sample
        .as_mut()
        .unwrap();
    sample.input_power_w = 0;
    sample.output_power_w = 0;
    app.graph.points = vec![Point {
        bucket_start_ms: app.timeline_now_ms(),
        input_power_w: 0.05,
        output_power_w: 0.05,
        sample_count: 20,
    }];
    for (width, height) in [(60, 19), (80, 24), (120, 30)] {
        let screen = text(&render(&mut app, width, height));
        assert!(screen.contains("INPUT 0 W") && screen.contains("OUTPUT 0 W"));
        assert_eq!(screen.matches('▁').count(), 2);
        assert!(!screen.contains('○'));
    }
    app.graph.points.clear();
    assert_eq!(text(&render(&mut app, 120, 30)).matches('○').count(), 2);
}

#[test]
fn idle_graphs_wait_for_history_and_resume_independently_on_new_power() {
    let mut app = App::new(false, None);
    let mut historical = status();
    let sample = historical.telemetry.sample.as_mut().unwrap();
    sample.input_power_w = 1;
    sample.output_power_w = 0;
    sample.received_at = (chrono::Utc::now() - chrono::Duration::seconds(60)).to_rfc3339();
    app.update(Event::Status(Box::new(historical)));
    seed_current_graph(&mut app);
    app.graph.points[0].bucket_start_ms -= 60_000;
    let mut idle = status();
    let sample = idle.telemetry.sample.as_mut().unwrap();
    sample.sequence = 2;
    sample.input_power_w = 0;
    sample.output_power_w = 0;
    app.update(Event::Status(Box::new(idle)));
    let screen = text(&render(&mut app, 94, 29));
    assert!(screen.contains("INPUT 0 W") && screen.contains("OUTPUT 0 W"));
    // Positive history delays the idle track even when the current reading is zero.
    assert_eq!(screen.matches('○').count(), 1);
    let circles = |buffer: &Buffer| {
        buffer
            .content
            .iter()
            .enumerate()
            .filter(|(_, cell)| cell.symbol() == "○")
            .map(|(index, _)| index % usize::from(buffer.area.width))
            .collect::<Vec<_>>()
    };
    assert!(circles(&render(&mut app, 94, 29))[0] > 47);
    app.graph.points[0].bucket_start_ms -= 440_000;
    app.prune_graph();
    assert_eq!(text(&render(&mut app, 94, 29)).matches('○').count(), 2);

    let mut current = status();
    let sample = current.telemetry.sample.as_mut().unwrap();
    sample.sequence = 3;
    sample.input_power_w = 0;
    sample.output_power_w = 42;
    app.update(Event::Status(Box::new(current)));
    seed_current_graph(&mut app);
    let screen = text(&render(&mut app, 94, 29));
    assert!(screen.contains("OUTPUT 42 W"));
    assert_eq!(screen.matches('○').count(), 1);
    assert!(circles(&render(&mut app, 94, 29))[0] < 47);
    // A positive current reading must suppress idle even before any graph samples exist.
    app.graph.points.clear();
    assert_eq!(text(&render(&mut app, 94, 29)).matches('○').count(), 1);
}

#[test]
fn idle_marker_moves_slowly_and_reverses_without_affecting_layout() {
    let mut app = app();
    let sample = app
        .status
        .as_mut()
        .unwrap()
        .telemetry
        .sample
        .as_mut()
        .unwrap();
    sample.input_power_w = 0;
    sample.output_power_w = 0;
    app.graph.points.clear();
    let mut origin = Vec::new();
    for (seconds, offset) in [(0, 0), (1, 0), (2, 1), (20, 10), (22, 9), (40, 0)] {
        app.animation_started = Instant::now() - Duration::from_secs(seconds);
        let buffer = render(&mut app, 94, 29);
        let positions: Vec<_> = buffer
            .content
            .iter()
            .enumerate()
            .filter(|(_, cell)| cell.symbol() == "○")
            .map(|(index, _)| index)
            .collect();
        assert_eq!(positions.len(), 2);
        if seconds == 0 {
            origin = positions.clone();
        }
        for (index, position) in positions.iter().enumerate() {
            assert_eq!(*position, origin[index] + offset);
        }
        assert!(app.controls.iter().all(|rect| rect.height == 2));
    }
    for (width, height) in [(60, 19), (80, 24), (120, 40)] {
        let screen = text(&render(&mut app, width, height));
        assert_eq!(screen.matches('○').count(), 2);
        assert!(screen.contains("INPUT 0 W") && screen.contains("OUTPUT 0 W"));
    }
    app.no_color = true;
    let buffer = render(&mut app, 60, 19);
    assert_eq!(text(&buffer).matches('○').count(), 2);
    assert!(
        buffer
            .content
            .iter()
            .all(|cell| cell.fg == ratatui::style::Color::Reset)
    );
}

#[test]
fn idle_marker_never_animates_missing_stale_or_disconnected_telemetry() {
    let mut app = app();
    let sample = app
        .status
        .as_mut()
        .unwrap()
        .telemetry
        .sample
        .as_mut()
        .unwrap();
    sample.input_power_w = 0;
    sample.output_power_w = 0;
    app.graph.points.clear();
    app.received -= Duration::from_secs(4);
    assert!(!text(&render(&mut app, 94, 29)).contains('○'));
    app.received = Instant::now();
    app.connected = false;
    assert!(!text(&render(&mut app, 94, 29)).contains('○'));
    app.connected = true;
    app.status.as_mut().unwrap().telemetry.state = "stale".into();
    assert!(!text(&render(&mut app, 94, 29)).contains('○'));
    app.status = None;
    assert!(!text(&render(&mut app, 94, 29)).contains('○'));
}

#[test]
fn frozen_clock_controls_freshness_history_animation_feedback_and_override_expiry() {
    use crate::clock::Clock;
    let now: chrono::DateTime<chrono::Utc> = "2026-10-05T12:00:00Z".parse().unwrap();
    let mut app = App::with_clock(
        false,
        Some(chrono_tz::UTC),
        Clock::Fixed {
            now,
            telemetry_elapsed: Duration::ZERO,
            animation_elapsed: Duration::from_secs(10),
            feedback_elapsed: Duration::from_secs(3),
        },
    );
    let mut current = status();
    current.server_time = now.to_rfc3339();
    current.logging =
        json!({"effective_level":"DEBUG", "override_expires_at":"2026-10-05T12:15:00Z"});
    let sample = current.telemetry.sample.as_mut().unwrap();
    sample.received_at = now.to_rfc3339();
    sample.input_power_w = 0;
    sample.output_power_w = 0;
    app.status = Some(current);
    app.connected = true;
    app.feedback = Some(Feedback::new("Fixed feedback", Severity::Success));
    let dashboard = render(&mut app, 120, 30);
    assert!(app.live() && text(&dashboard).contains('○'));
    app.received -= Duration::from_secs(86_400);
    app.animation_started -= Duration::from_secs(37);
    app.feedback.as_mut().unwrap().started -= Duration::from_secs(86_400);
    assert_eq!(dashboard, render(&mut app, 120, 30));
    assert!(app.live());
    app.view = View::Logs;
    assert!(text(&render(&mut app, 120, 30)).contains("until 2026-10-05 12:15:00"));
    assert_eq!(app.logs.day.to_string(), "2026-10-05");
}

#[test]
fn settings_and_help_are_bounded_modal_overlays_with_inactive_dashboard_hitboxes() {
    let mut app = app();
    app.server_url = "https://mypowers.lxc.efez.net".into();
    let normal = render(&mut app, 94, 29);
    let station_color = normal[(2, 1)].fg;
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert!(app.view == View::Settings);
    app.settings_tab = SettingsTab::Debug;
    for (width, height) in [(60, 19), (80, 24), (94, 29)] {
        let screen = text(&render(&mut app, width, height));
        assert!(screen.contains("SETTINGS") && screen.contains("Debug / Diagnostics"));
        assert!(screen.contains("Server") && screen.contains("API connection"));
        assert!(screen.contains("https://mypowers.lxc.efez.net"));
        assert!(screen.contains("A8:3B:76:E6:D4:A0") && screen.contains("Runtime logging"));
        assert!(app.settings_actions.iter().all(|rect| !rect.is_empty()));
        assert_eq!(app.settings_actions.map(|rect| rect.width), [13, 13, 17]);
        let buttons_y = app.settings_actions[0].y;
        assert!(app.settings_actions.iter().all(|rect| rect.y == buttons_y));
        assert_eq!(buttons_y, height - 5);
        assert!(app.controls.iter().all(|rect| rect.is_empty()) && app.title.is_empty());
    }
    let settings = render(&mut app, 94, 29);
    assert_ne!(settings[(2, 1)].fg, station_color);
    assert!(text(&settings).contains("hci0") && text(&settings).contains("Runtime logging"));
    app.settings_selected = 2;
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Effect::Request(Intent::Debug(true))
    ));
    app.key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    let help = render(&mut app, 94, 29);
    assert!(text(&help).contains("HELP — SETTINGS") && text(&help).contains("AP S300 V2.0"));
    assert_ne!(help[(2, 1)].fg, station_color);
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.view == View::Dashboard);
}

#[test]
fn persistent_status_indicators_include_only_nonzero_counts() {
    let mut app = app();
    app.feedback = None;
    for (warnings, errors, expected) in [
        (0, 0, ""),
        (1, 0, "warn:1"),
        (0, 2, "err:2"),
        (2, 1, "warn:2 • err:1"),
    ] {
        app.warning_count = warnings;
        app.error_count = errors;
        let screen = text(&render(&mut app, 94, 29));
        assert_eq!(screen.lines().last().unwrap().trim(), expected);
    }
}

#[test]
fn graph_scales_start_at_100_and_double_only_when_exceeded() {
    use crate::history::power_scale;
    for (peak, expected) in [
        (0.0, 100),
        (0.05, 100),
        (100.0, 100),
        (100.01, 200),
        (200.0, 200),
        (200.01, 400),
        (800.0, 800),
        (65535.0, 102400),
    ] {
        assert_eq!(power_scale([0.0, peak]), expected);
    }
    assert_eq!(power_scale([]), 100);
}

#[test]
fn graph_switch_is_contextual_session_local_and_preserves_loaded_averages() {
    use crate::history::Visualization;
    let mut app = fixed_graph_app();
    seed_current_graph(&mut app);
    let before = serde_json::to_value(app.status.as_ref().unwrap()).unwrap();
    let point = app.graph.points[0].clone();
    let first = app.history_request().unwrap();
    for view in [View::Logs, View::Settings, View::Help, View::Quit] {
        app.view = view;
        assert!(matches!(
            app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)),
            Effect::None
        ));
        assert_eq!(app.graph.visualization, Visualization::Sparkline);
    }
    app.view = View::Dashboard;
    for modifier in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        app.key(KeyEvent::new(KeyCode::Char('g'), modifier));
        assert_eq!(app.graph.visualization, Visualization::Sparkline);
    }
    app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(app.graph.visualization, Visualization::Chart);
    assert_eq!(app.graph.points[0].bucket_start_ms, point.bucket_start_ms);
    assert_eq!(app.graph.points[0].input_power_w, point.input_power_w);
    let screen = text(&render(&mut app, 94, 29));
    assert!(screen.contains("g spark") && screen.contains("   200│"));
    assert_eq!(
        app.graph.width, 83,
        "The shared chart excludes the reserved Y-axis columns"
    );
    let expanded = app.history_request().unwrap();
    assert!(expanded.generation > first.generation);
    assert_eq!(expanded.width, 83);
    app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(app.graph.visualization, Visualization::Sparkline);
    render(&mut app, 94, 29);
    assert_eq!(app.graph.width, 43);
    assert_eq!(
        serde_json::to_value(app.status.as_ref().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        App::new(false, None).graph.visualization,
        Visualization::Sparkline
    );
}

#[test]
fn chart_lines_preserve_zero_buckets_and_break_across_missing_observations() {
    let mut app = fixed_graph_app();
    let now = app.timeline_now_ms();
    app.graph.points = vec![
        Point {
            bucket_start_ms: now - 40_000,
            input_power_w: 0.0,
            output_power_w: 3.0,
            sample_count: 1,
        },
        Point {
            bucket_start_ms: now - 30_000,
            input_power_w: 0.5,
            output_power_w: 2.0,
            sample_count: 2,
        },
        Point {
            bucket_start_ms: now,
            input_power_w: 63.0,
            output_power_w: 181.0,
            sample_count: 1,
        },
    ];
    let columns = app.graph.columns(now, 6, false);
    assert_eq!(
        columns,
        vec![None, Some(0.0), Some(0.5), None, None, Some(63.0)]
    );
    assert_eq!(
        ui::chart_runs(&columns),
        vec![vec![(1.0, 0.0), (2.0, 0.5)], vec![(5.0, 63.0)]]
    );
    assert!(ui::chart_runs(&[None, None]).is_empty());
    assert!(app.graph.columns(now, 0, false).is_empty());
}

#[test]
fn shared_chart_uses_seven_rows_two_colors_common_scale_and_safe_minimum_layout() {
    use crate::history::Visualization;
    use ratatui::style::Color;
    let green = Color::Rgb(118, 203, 137);
    let cyan = Color::Rgb(92, 181, 204);
    let mut app = fixed_graph_app();
    app.graph.visualization = Visualization::Chart;
    let now = app.timeline_now_ms();
    app.graph.points = (0..90)
        .map(|i| Point {
            bucket_start_ms: now - (89 - i) * 10_000,
            input_power_w: (i % 10 + 1) as f64 * 8.0,
            output_power_w: (i % 10 + 1) as f64 * 30.0,
            sample_count: 1,
        })
        .collect();
    for (width, height) in [(60, 19), (80, 24), (120, 30)] {
        let plot_height = if height == 19 { 6 } else { 7 };
        let buffer = render(&mut app, width, height);
        let screen = text(&buffer);
        assert!(screen.contains("INPUT 63 W") && screen.contains("OUTPUT 181 W"));
        assert!(
            !screen.contains("0–400 W"),
            "The Y axis already identifies the scale"
        );
        let row = screen
            .lines()
            .position(|line| line.contains("INPUT"))
            .unwrap() as u16;
        for color in [green, cyan] {
            let cells: Vec<_> = (row + 2..row + 2 + plot_height)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    buffer[(x, y)].fg == color
                        && buffer[(x, y)]
                            .symbol()
                            .chars()
                            .any(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
                })
                .collect();
            assert!(
                !cells.is_empty(),
                "Both datasets must render at {width}x{height}"
            );
        }
        assert!(
            app.controls
                .iter()
                .all(|rect| rect.y >= row + plot_height + 4 && rect.bottom() < height)
        );
        assert!(screen.contains("q quit"));
        assert!(
            screen.contains("   400│") && screen.contains("   200│") && screen.contains("     0└")
        );
        assert!(
            screen
                .lines()
                .nth(usize::from(row + plot_height + 2))
                .unwrap()
                .contains("     0└")
        );
        assert!(!screen.contains("     0│"));
        assert!(
            screen
                .lines()
                .nth(usize::from(row + plot_height + 3))
                .unwrap()
                .contains("12:00")
        );
        assert_eq!(app.graph.width, width.min(94) - 11);
    }
    app.connected = false;
    let buffer = render(&mut app, 120, 30);
    assert!(
        buffer
            .content
            .iter()
            .filter(|cell| cell
                .symbol()
                .chars()
                .any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)))
            .all(|cell| cell.fg == Color::Rgb(79, 93, 102))
    );
    assert!(text(&buffer).contains("DAEMON OFFLINE"));
    assert!(text(&render(&mut app, 50, 14)).contains("Terminal too small"));
}

#[test]
fn shared_chart_idle_waits_for_both_histories_and_returns_on_fractional_power() {
    use crate::history::Visualization;
    let mut app = fixed_graph_app();
    app.graph.visualization = Visualization::Chart;
    let sample = app
        .status
        .as_mut()
        .unwrap()
        .telemetry
        .sample
        .as_mut()
        .unwrap();
    sample.input_power_w = 0;
    sample.output_power_w = 0;
    assert_eq!(text(&render(&mut app, 94, 29)).matches('○').count(), 1);
    seed_current_graph(&mut app);
    app.graph.points[0].output_power_w = 0.05;
    let buffer = render(&mut app, 94, 29);
    assert!(!text(&buffer).contains('○'));
    assert!(buffer.content.iter().any(|cell| {
        cell.symbol()
            .chars()
            .any(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
    }));
    app.graph.points.clear();
    assert_eq!(text(&render(&mut app, 94, 29)).matches('○').count(), 1);
    app.connected = false;
    assert!(!text(&render(&mut app, 94, 29)).contains('○'));
}

#[test]
fn chart_time_ticks_align_to_full_minutes_and_move_with_history() {
    let now = "2026-10-05T12:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap()
        .timestamp_millis();
    for (resolution, expected) in [
        (
            Resolution::TenSeconds,
            vec![(10, "11:48"), (34, "11:52"), (58, "11:56"), (82, "12:00")],
        ),
        (
            Resolution::Minute,
            vec![(1, "10:39"), (28, "11:06"), (55, "11:33"), (82, "12:00")],
        ),
        (
            Resolution::Hour,
            vec![
                (1, "10-02 03h"),
                (28, "10-03 06h"),
                (55, "10-04 09h"),
                (82, "10-05 12h"),
            ],
        ),
    ] {
        let actual = ui::chart_time_ticks(now + 750, resolution, 83, Some(chrono_tz::UTC));
        assert_eq!(
            actual,
            expected
                .into_iter()
                .map(|(column, label)| (column, label.into()))
                .collect::<Vec<_>>()
        );
        let moved = ui::chart_time_ticks(
            now + resolution.seconds() * 1000,
            resolution,
            83,
            Some(chrono_tz::UTC),
        );
        assert_eq!(
            moved,
            actual
                .into_iter()
                .map(|(column, label)| (column - 1, label))
                .collect::<Vec<_>>()
        );
    }
    for width in [49, 69, 83] {
        for offset in 0..36 {
            let ticks = ui::chart_time_ticks(
                now + offset * 10_000,
                Resolution::TenSeconds,
                width,
                Some(chrono_tz::UTC),
            );
            assert!(
                (3..=4).contains(&ticks.len()),
                "3–4 ticks at every phase of the window"
            );
            let distances: Vec<_> = ticks.windows(2).map(|pair| pair[1].0 - pair[0].0).collect();
            assert!(distances.iter().all(|distance| *distance == distances[0]));
            assert!(ticks.iter().all(|(_, label)| label.len() == 5));
        }
    }
    assert_eq!(
        ui::chart_time_ticks(now, Resolution::Minute, 3, Some(chrono_tz::Europe::Warsaw)),
        vec![
            (0, "13:58".into()),
            (1, "13:59".into()),
            (2, "14:00".into())
        ]
    );
    let dst = "2026-10-25T01:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap()
        .timestamp_millis();
    assert_eq!(
        ui::chart_time_ticks(dst, Resolution::Minute, 3, Some(chrono_tz::Europe::Warsaw)),
        vec![
            (0, "02:58".into()),
            (1, "02:59".into()),
            (2, "02:00".into())
        ]
    );
}

#[test]
fn stable_separated_chart_series_do_not_drop_columns_and_missing_buckets_align() {
    use crate::history::Visualization;
    use ratatui::style::Color;
    for missing in [false, true] {
        let mut app = fixed_graph_app();
        app.graph.visualization = Visualization::Chart;
        let sample = app
            .status
            .as_mut()
            .unwrap()
            .telemetry
            .sample
            .as_mut()
            .unwrap();
        sample.input_power_w = 51;
        sample.output_power_w = 28;
        let now = app.timeline_now_ms();
        app.graph.points = (0..83)
            .filter(|i| !missing || (!(20..30).contains(i) && *i != 55))
            .map(|i| Point {
                bucket_start_ms: now - (82 - i) * 10_000,
                input_power_w: 51.0,
                output_power_w: 28.0,
                sample_count: 1,
            })
            .collect();
        let buffer = render(&mut app, 94, 29);
        let row = text(&buffer)
            .lines()
            .position(|line| line.contains("INPUT"))
            .unwrap() as u16
            + 2;
        for column in 0..83 {
            let expected = !missing || (!(20..30).contains(&column) && column != 55);
            for color in [Color::Rgb(118, 203, 137), Color::Rgb(92, 181, 204)] {
                let visible = (row..row + 7).any(|y| {
                    buffer[(9 + column, y)].fg == color
                        && buffer[(9 + column, y)]
                            .symbol()
                            .chars()
                            .any(|c| ('\u{2801}'..='\u{28ff}').contains(&c))
                });
                assert_eq!(
                    visible, expected,
                    "Same gap mask for both separated series at column {column}"
                );
            }
        }
    }
}

#[test]
fn shared_chart_unions_braille_patterns_when_input_crosses_a_cell_boundary() {
    use crate::history::Visualization;
    use ratatui::style::Color;
    let draw = |show_input: bool, show_output: bool| {
        let mut app = fixed_graph_app();
        app.graph.visualization = Visualization::Chart;
        let sample = app
            .status
            .as_mut()
            .unwrap()
            .telemetry
            .sample
            .as_mut()
            .unwrap();
        sample.input_power_w = 29;
        sample.output_power_w = 25;
        let now = app.timeline_now_ms();
        app.graph.points = (0..83)
            .map(|i| Point {
                bucket_start_ms: now - (82 - i) * 10_000,
                input_power_w: if show_input {
                    if i % 2 == 0 { 27.0 } else { 29.0 }
                } else {
                    0.0
                },
                output_power_w: if show_output { 25.0 } else { 0.0 },
                sample_count: 1,
            })
            .collect();
        render(&mut app, 94, 29)
    };
    let input = draw(true, false);
    let output = draw(false, true);
    let both = draw(true, true);
    let row = text(&both)
        .lines()
        .position(|line| line.contains("INPUT"))
        .unwrap() as u16
        + 2;
    let pattern = |cell: &ratatui::buffer::Cell| {
        cell.symbol()
            .chars()
            .next()
            .filter(|c| ('\u{2801}'..='\u{28ff}').contains(c))
            .map_or(0, |c| c as u32 - 0x2800)
    };
    let mut shared = 0;
    for x in 9..92 {
        // Exclude the dummy zero lines on the last plot row.
        for y in row..row + 6 {
            let a = pattern(&input[(x, y)]);
            let b = pattern(&output[(x, y)]);
            assert_eq!(
                pattern(&both[(x, y)]),
                a | b,
                "Neither series may erase the other's dots"
            );
            if a > 0 && b > 0 {
                shared += 1;
                assert_eq!(both[(x, y)].fg, Color::Rgb(224, 232, 236));
            }
        }
    }
    assert!(
        shared > 30,
        "Exercise the actual 27/29 W versus 25 W collision"
    );
}

#[test]
fn local_theme_choice_works_offline_without_changing_server_settings() {
    let path = std::env::temp_dir().join(format!("mypowers-theme-{}.toml", uuid::Uuid::new_v4()));
    let mut app = app();
    app.client_preferences.path = Some(path.clone());
    app.view = View::Settings;
    app.settings_tab = SettingsTab::Preferences;
    app.settings_selected = 2;
    app.connected = false;
    app.settings = None;
    let server_settings = app.settings_draft.clone();
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Effect::None
    ));
    assert_eq!(
        app.settings_picker.as_ref().unwrap().field,
        crate::settings::Field::Theme
    );
    app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Effect::None
    ));
    assert_eq!(app.client_preferences.theme, "Catppuccin");
    assert_eq!(app.settings_draft, server_settings);
    assert!(app.pending.is_none());
    assert_eq!(
        crate::client_ui::ClientPreferences::load(path.clone(), true)
            .unwrap()
            .theme,
        "Catppuccin"
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn every_theme_applies_to_every_view_and_the_background_behind_modals() {
    use crate::history::Visualization;
    use ratatui::style::Color;
    for name in crate::client_ui::THEMES {
        let mut app = fixed_graph_app();
        app.client_preferences.theme = name.into();
        app.settings = Some(crate::settings::Settings::default());
        app.settings_draft = app.settings.clone().unwrap();
        app.feedback = None;
        let theme = app.client_preferences.theme();
        for (width, height) in [(60, 19), (94, 29), (120, 30), (50, 14)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for view in [
                View::Dashboard,
                View::Logs,
                View::Settings,
                View::Help,
                View::Quit,
                View::Dashboard,
            ] {
                app.view = view;
                for visualization in [Visualization::Sparkline, Visualization::Chart] {
                    app.graph.visualization = visualization;
                    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
                    let buffer = terminal.backend().buffer();
                    assert_eq!(
                        buffer[(0, 0)].bg,
                        theme.background,
                        "{name} {view:?} {visualization:?}"
                    );
                    assert_eq!(
                        *buffer,
                        render(&mut app, width, height),
                        "theme transition must clear previous colors"
                    );
                    if width >= 60 && view == View::Settings {
                        assert!(text(buffer).contains(name));
                        assert!(app.settings_tabs.iter().all(|rect| !rect.is_empty()));
                    }
                    app.no_color = true;
                    let plain = render(&mut app, width, height);
                    assert!(
                        plain
                            .content
                            .iter()
                            .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
                    );
                    app.no_color = false;
                }
            }
        }
    }
}

#[test]
fn custom_palette_reaches_all_chrome_and_changes_immediately_on_a_reused_terminal() {
    use ratatui::style::Color;
    let mut app = fixed_graph_app();
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    for view in [
        View::Dashboard,
        View::Logs,
        View::Settings,
        View::Help,
        View::Quit,
    ] {
        app.view = view;
        app.client_preferences = Default::default();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        app.client_preferences.colors = std::collections::BTreeMap::from([
            ("background".into(), "#112233".into()),
            ("foreground".into(), "#aabbcc".into()),
            ("border".into(), "#778899".into()),
            ("muted".into(), "#8899aa".into()),
            ("focus".into(), "#334455".into()),
        ]);
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(0x11, 0x22, 0x33));
        for old in [
            Color::Rgb(16, 21, 27),
            Color::Rgb(224, 232, 236),
            Color::Rgb(119, 144, 153),
            Color::Rgb(68, 94, 105),
            Color::Rgb(79, 93, 102),
            Color::Rgb(34, 46, 54),
        ] {
            assert!(
                buffer
                    .content
                    .iter()
                    .all(|cell| cell.bg != old && cell.fg != old),
                "old chrome remains in {view:?}: {old:?}"
            );
        }
        assert_eq!(*buffer, render(&mut app, 120, 30));
    }
}

#[test]
fn all_settings_tabs_and_value_dialogs_share_the_active_theme() {
    use crate::settings::{Field, Picker, Settings, SettingsTab};
    for name in crate::client_ui::THEMES {
        let mut app = fixed_graph_app();
        app.client_preferences.theme = name.into();
        app.settings = Some(Settings::default());
        app.settings_draft = Settings::default();
        app.view = View::Settings;
        let theme = app.client_preferences.theme();
        for tab in SettingsTab::ALL {
            app.settings_tab = tab;
            for (width, height) in [(60, 19), (120, 30)] {
                let buffer = render(&mut app, width, height);
                assert_eq!(buffer[(0, 0)].bg, theme.background);
                let title_cell = buffer
                    .content
                    .iter()
                    .find(|cell| cell.symbol() == "S" && cell.fg == theme.foreground);
                assert!(title_cell.is_some(), "Settings title uses theme foreground");
            }
        }
        for field in Field::PREFERENCES
            .into_iter()
            .chain(Field::CHARTS)
            .chain(Field::ALERTS)
            .filter(|field| !field.is_segmented())
        {
            app.settings_picker = Some(Picker {
                field,
                selected: 0,
                query: String::new(),
            });
            for (width, height) in [(60, 19), (120, 30)] {
                let buffer = render(&mut app, width, height);
                assert_eq!(buffer[(0, 0)].bg, theme.background);
                assert!(text(&buffer).contains("Enter select"));
                assert!(!app.settings_choices.is_empty());
            }
        }
    }
}

#[test]
fn segmented_settings_preserve_saved_choices_and_support_keyboard_mouse_and_autosave() {
    use crate::history::Visualization;
    use crate::settings::{Field, SettingsTab};
    for (field, tab, row) in [
        (Field::Visualization, SettingsTab::Charts, 0),
        (Field::Scale, SettingsTab::Charts, 2),
        (Field::AlertEnabled, SettingsTab::Alerts, 0),
    ] {
        for theme in crate::client_ui::THEMES {
            for (width, height) in [(60, 19), (120, 30)] {
                let mut app = app();
                let mut saved = Settings {
                    graph_visualization: Visualization::Chart,
                    graph_base_scale_w: 300,
                    ..Settings::default()
                };
                saved.battery_alert.enabled = false;
                app.update(Event::Settings(saved.clone()));
                app.client_preferences.theme = theme.into();
                app.view = View::Settings;
                app.settings_tab = tab;
                app.settings_selected = row;
                let buffer = render(&mut app, width, height);
                let segments: Vec<_> = app
                    .settings_segments
                    .iter()
                    .filter(|(_, f, _)| *f == field)
                    .copied()
                    .collect();
                assert_eq!(segments.len(), field.choices().len());
                assert_eq!(app.settings_draft.value(field), field.choices()[1]);
                assert_ne!(
                    buffer[(segments[0].0.x, segments[0].0.y)].bg,
                    buffer[(segments[1].0.x, segments[1].0.y)].bg
                );
                for (_, _, choice) in &segments {
                    assert!(text(&buffer).contains(&field.segment_label(*choice)));
                }
                app.no_color = true;
                let plain = render(&mut app, width, height);
                assert!(
                    plain[(segments[1].0.x, segments[1].0.y)]
                        .modifier
                        .contains(ratatui::style::Modifier::REVERSED)
                );
                assert!(
                    !plain[(segments[0].0.x, segments[0].0.y)]
                        .modifier
                        .contains(ratatui::style::Modifier::REVERSED)
                );
                app.no_color = false;
                assert!(matches!(
                    app.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
                    Effect::None
                ));
                assert_eq!(app.settings_draft.value(field), field.choices()[0]);
                assert!(app.pending.is_none() && app.settings_picker.is_none());
                assert_eq!(app.settings, Some(saved.clone()));
                render(&mut app, width, height);
                let rect = app
                    .settings_segments
                    .iter()
                    .find(|(_, f, choice)| *f == field && *choice == 1)
                    .unwrap()
                    .0;
                assert!(matches!(
                    app.mouse(MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column: rect.x,
                        row: rect.y,
                        modifiers: KeyModifiers::NONE
                    }),
                    Effect::None
                ));
                assert_eq!(app.settings_draft.value(field), field.choices()[1]);
                assert!(matches!(
                    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                    Effect::None
                ));
                assert_eq!(app.settings_draft.value(field), field.choices()[0]);
                assert_eq!(app.settings, Some(saved.clone()));
                assert!(matches!(
                    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
                    Effect::Request(Intent::SaveSettings(_))
                ));
            }
        }
    }
}

#[test]
fn segmented_fields_stay_disabled_when_settings_are_missing_or_an_operation_is_pending() {
    let mut app = app();
    app.view = View::Settings;
    app.settings_tab = crate::settings::SettingsTab::Charts;
    for pending in [false, true] {
        app.settings = pending.then(Settings::default);
        app.pending = pending.then(|| "settings request".into());
        let draft = app.settings_draft.clone();
        render(&mut app, 60, 19);
        let rect = app.settings_segments[1].0;
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.settings_draft, draft);
        assert!(app.settings_picker.is_none());
    }
    assert!(!crate::settings::Field::Interval.is_segmented());
}

#[test]
fn settings_autosave_debounces_retries_and_flushes_on_exit() {
    use crate::clock::Clock;
    let mut app = app();
    let start = app.clock.now();
    let advance = |app: &mut App, seconds| {
        app.clock = Clock::Fixed {
            now: start + chrono::Duration::seconds(seconds),
            telemetry_elapsed: Duration::ZERO,
            animation_elapsed: Duration::ZERO,
            feedback_elapsed: Duration::ZERO,
        };
    };
    advance(&mut app, 0);
    app.update(Event::Settings(Settings::default()));
    app.view = View::Settings;
    app.settings_tab = SettingsTab::Charts;
    app.settings_selected = 0;
    app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(matches!(app.autosave(), Effect::None));
    advance(&mut app, 4);
    app.settings_selected = 2;
    app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    advance(&mut app, 5);
    assert!(matches!(app.autosave(), Effect::None));
    advance(&mut app, 9);
    let Effect::Request(Intent::SaveSettings(draft)) = app.autosave() else {
        panic!("Expected debounced save");
    };
    assert_eq!(draft.graph_base_scale_w, 300);
    assert!(matches!(app.autosave(), Effect::None));
    app.update(Event::Finished(Feedback::new(
        "Save failed",
        Severity::Error,
    )));
    advance(&mut app, 13);
    assert!(matches!(app.autosave(), Effect::None));
    advance(&mut app, 14);
    assert!(matches!(
        app.autosave(),
        Effect::Request(Intent::SaveSettings(_))
    ));
    app.update(Event::Settings(draft));
    app.update(Event::Finished(Feedback::new(
        "Settings saved",
        Severity::Success,
    )));
    assert!(matches!(app.autosave(), Effect::None));
    app.settings_selected = 0;
    app.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    // Re-selecting the same value does not postpone a pending save.
    let due = app.settings_save_due;
    advance(&mut app, 15);
    app.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.settings_save_due, due);
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        Effect::Request(Intent::SaveSettings(_))
    ));
    assert!(app.quit_after_settings);
    app.update(Event::Finished(Feedback::new(
        "Save failed",
        Severity::Error,
    )));
    assert!(!app.quit_after_settings && app.settings_dirty());
    // Offline drafts survive; reconnect resumes the overdue save.
    app.connected = false;
    advance(&mut app, 30);
    assert!(matches!(app.autosave(), Effect::None));
    app.connected = true;
    assert!(matches!(
        app.autosave(),
        Effect::Request(Intent::SaveSettings(_))
    ));
}

#[test]
fn settings_have_no_save_button_or_waiting_notice_in_any_theme() {
    for theme in crate::client_ui::THEMES {
        for tab in [
            SettingsTab::Preferences,
            SettingsTab::Charts,
            SettingsTab::Alerts,
        ] {
            let mut app = app();
            app.update(Event::Settings(Settings::default()));
            app.view = View::Settings;
            app.settings_tab = tab;
            app.client_preferences.theme = theme.into();
            app.feedback = None;
            app.settings_draft.battery_alert.enabled = false;
            for size in [(60, 19), (120, 30)] {
                let screen = text(&render(&mut app, size.0, size.1));
                assert!(!screen.contains("Save changes") && !screen.contains("Waiting"));
                assert!(app.settings_actions.iter().all(|r| r.width == 0));
            }
        }
    }
}

#[test]
fn reverting_settings_cancels_autosave_and_pending_write_blocks_picker_confirmation() {
    let mut app = app();
    app.update(Event::Settings(Settings::default()));
    app.view = View::Settings;
    app.settings_tab = SettingsTab::Charts;
    app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(app.settings_save_due.is_some());
    app.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert!(app.settings_save_due.is_none() && !app.settings_dirty());
    app.settings_selected = 1;
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.pending = Some("settings request".into());
    let draft = app.settings_draft.clone();
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.settings_draft, draft);
    assert!(app.settings_picker.is_some());
}

#[test]
fn quit_shortcuts_flush_settings_immediately_even_from_a_picker() {
    for ctrl in [false, true] {
        for picker in [false, true] {
            let mut app = app();
            app.update(Event::Settings(Settings::default()));
            app.view = View::Settings;
            app.settings_tab = SettingsTab::Charts;
            app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
            let expected = app.settings_draft.clone();
            if picker {
                app.settings_selected = 1;
                app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            }
            let modifiers = if ctrl {
                KeyModifiers::CONTROL
            } else {
                KeyModifiers::NONE
            };
            let Effect::Request(Intent::SaveSettings(draft)) =
                app.key(KeyEvent::new(KeyCode::Char('q'), modifiers))
            else {
                panic!("Quit shortcut must flush the changed settings immediately");
            };
            assert_eq!(draft, expected);
            assert_eq!(app.quit_after_settings, ctrl);
            if !ctrl {
                assert!(app.view == View::Quit);
                assert!(matches!(
                    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                    Effect::None
                ));
                assert!(app.quit_after_settings);
            }
            app.update(Event::Settings(draft));
            app.update(Event::Finished(Feedback::new(
                "Settings saved",
                Severity::Success,
            )));
            assert!(app.quit_after_settings && !app.settings_dirty() && app.pending.is_none());
        }
    }
}

#[test]
fn segmented_field_labels_keep_the_same_style_when_focused() {
    for name in crate::client_ui::THEMES {
        for (tab, row) in [
            (SettingsTab::Charts, 0),
            (SettingsTab::Charts, 2),
            (SettingsTab::Alerts, 0),
        ] {
            let mut app = app();
            app.update(Event::Settings(Settings::default()));
            app.client_preferences.theme = name.into();
            app.view = View::Settings;
            app.settings_tab = tab;
            for (width, height) in [(60, 19), (120, 30)] {
                app.settings_selected = row;
                let focused = render(&mut app, width, height);
                let rect = app.settings_fields[row].0;
                app.settings_selected = 1;
                let unfocused = render(&mut app, width, height);
                for x in rect.x..rect.x + 20 {
                    let selected = &focused[(x, rect.y)];
                    let other = &unfocused[(x, rect.y)];
                    assert_eq!(selected, other, "Label changed with focus in {name}");
                    assert!(!selected.modifier.contains(ratatui::style::Modifier::BOLD));
                }
            }
        }
    }
}

#[test]
fn dashboard_l_opens_logs_without_changing_outputs_and_old_control_shortcuts_are_inert() {
    for code in [KeyCode::Char('a'), KeyCode::Char('d')] {
        let mut app = app();
        assert!(matches!(
            app.key(KeyEvent::new(code, KeyModifiers::NONE)),
            Effect::None
        ));
        assert!(app.pending.is_none() && app.view == View::Dashboard);
    }
    let mut app = app();
    let snapshot = serde_json::to_value(&app.status).unwrap();
    let screen = text(&render(&mut app, 120, 30));
    assert!(screen.contains("l logs") && screen.contains("s settings"));
    assert!(!screen.contains("a AC") && !screen.contains("l light") && !screen.contains("F3 logs"));
    assert!(screen.contains("AC") && screen.contains("DC") && screen.contains("LIGHT"));
    assert!(matches!(
        app.key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE)),
        Effect::Logs(_)
    ));
    assert!(app.view == View::Logs && app.pending.is_none());
    assert_eq!(serde_json::to_value(&app.status).unwrap(), snapshot);
}

#[test]
fn footer_hotkeys_use_foreground_in_each_menu_without_the_theme_accent() {
    for name in crate::client_ui::THEMES {
        for view in [
            View::Dashboard,
            View::Settings,
            View::Logs,
            View::Help,
            View::Quit,
        ] {
            let mut app = app();
            app.update(Event::Settings(Settings::default()));
            app.client_preferences.theme = name.into();
            app.view = view;
            app.feedback = None;
            let foreground = app.client_preferences.theme().foreground;
            let buffer = render(&mut app, 120, 30);
            let expected = match view {
                View::Dashboard => "l logs",
                View::Settings => "Tab tabs",
                View::Logs | View::Help => "Esc close",
                View::Quit => "Enter select",
            };
            let mut found = false;
            for y in (0..30).rev() {
                let row = (0..120)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>();
                if let Some(start) = row.find(expected) {
                    let x = row[..start].chars().count() as u16;
                    assert_eq!(buffer[(x, y)].fg, foreground, "{name}: {expected}");
                    found = true;
                    break;
                }
            }
            assert!(found, "Missing {expected}");
        }
    }
}

#[test]
fn settings_title_double_click_copies_displayed_settings_without_saving() {
    let mut app = app();
    app.update(Event::Settings(Settings::default()));
    app.view = View::Settings;
    app.client_preferences.theme = "Nord".into();
    app.settings_draft.battery_alert.threshold_percent = 22;
    for (width, height) in [(60, 19), (120, 30)] {
        app.resize();
        render(&mut app, width, height);
        let rect = app.settings_title;
        assert_eq!(rect.width, 8);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 2,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        assert!(matches!(app.mouse(click), Effect::None));
        assert!(matches!(app.mouse(click), Effect::CopySettings));
        assert!(app.pending.is_none());
        let copied = app.settings_clipboard_json();
        assert_eq!(copied["settings"]["battery_alert"]["threshold_percent"], 22);
        assert_eq!(copied["client"]["theme"], "Nord");
        assert_eq!(copied["unsaved_changes"], true);
        app.mouse(click);
        app.resize();
        render(&mut app, width, height);
        assert!(matches!(app.mouse(click), Effect::None));
    }
    app.settings = None;
    assert!(app.settings_clipboard_json()["settings"].is_null());
    app.settings_picker = Some(crate::settings::Picker {
        field: crate::settings::Field::Theme,
        selected: 0,
        query: String::new(),
    });
    render(&mut app, 120, 30);
    assert_eq!(app.settings_title, ratatui::layout::Rect::default());
}

#[test]
fn bluetooth_alert_section_is_separate_visible_and_autosaved() {
    for (width, height) in [(60, 19), (120, 30)] {
        let mut app = app();
        app.update(Event::Settings(Settings::default()));
        app.view = View::Settings;
        app.settings_tab = SettingsTab::Alerts;
        app.settings_selected = 4;
        let screen = text(&render(&mut app, width, height));
        for label in [
            "Battery alerts",
            "Bluetooth connection",
            "Connection alert",
            "En",
            "Dis",
            "Alert after",
            "60 s",
            "Stable recovery",
            "15 s",
        ] {
            assert!(
                screen.contains(label),
                "Missing {label} at {width}x{height}: {screen}"
            );
        }
        assert!(screen.find("Bluetooth connection").unwrap() > screen.find("Hysteresis").unwrap());
        let original_battery = app.settings_draft.battery_alert.clone();
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(!app.settings_draft.connection_alert.enabled);
        assert_eq!(app.settings_draft.battery_alert, original_battery);
        assert!(matches!(
            app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Effect::Request(Intent::SaveSettings(_))
        ));
    }
}
