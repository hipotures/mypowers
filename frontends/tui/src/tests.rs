use crate::{
    app::{App, Effect, View},
    feedback::{Feedback, Severity},
    model::{Command, Status},
    network::{ClipboardTarget, Event, Intent},
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use serde_json::json;
use std::time::{Duration, Instant};

pub(crate) fn status() -> Status {
    serde_json::from_value(json!({
        "schema_version":1,"server_time":chrono::Utc::now().to_rfc3339(),"server_instance_id":"88767477-2a2a-481f-843b-30d56a5e3f10","state_version":1,
        "device":{"name":"AP S300 V2.0"},
        "connection":{"phase":"connected","desired":"running","link_connected":true,"session_id":"session","message":"Connected","adapter_id":"hci0"},
        "telemetry":{"state":"live","age_seconds":0.0,"sample":{
            "sequence":1,"received_at":chrono::Utc::now().to_rfc3339(),"segment_id":"segment",
            "battery_percent":78,"input_power_w":63,"output_power_w":181,"remaining_minutes":2937,
            "ac_enabled":false,"dc_enabled":false,"light_enabled":false}},
        "controls":{"allowed":true,"outputs_revision":2,"pending_command_id":null,"reason_code":null},
        "history":{"state":"ok"},"logging":{"effective_level":"INFO"}
    })).unwrap()
}

fn app() -> App {
    let mut app = App::new(false, None);
    app.update(Event::Status(Box::new(status())));
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
    app.update(Event::Finished(
        "Outcome uncertain".into(),
        Feedback::new("Command outcome uncertain", Severity::Warning),
    ));
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
        for view in [View::Logs, View::Help] {
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
            assert!(row.starts_with("AC ON confirmed"));
            assert!(row["AC ON confirmed".len()..].trim().is_empty());
            colors.push(buffer[(0, 28)].fg);
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
    assert!(
        text(&render(&mut app, 60, 19))
            .ends_with("New message                                                 ")
    );
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
            )
        })
        .unwrap();
    assert_eq!(terminal.backend().buffer()[(4, 0)].symbol(), "…");
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
    assert_eq!(app.feedback.as_ref().unwrap().message, "Lamps ON confirmed");
    assert_eq!(app.feedback.as_ref().unwrap().severity, Severity::Success);
    let started = app.feedback.as_ref().unwrap().started;
    app.update(Event::Command(command.clone()));
    assert_eq!(app.feedback.as_ref().unwrap().started, started);
    command.status = "rejected".into();
    command.reason_code = Some("telemetry_unavailable".into());
    app.update(Event::Command(command));
    assert_eq!(
        app.feedback.as_ref().unwrap().message,
        "Lamps ON failed: telemetry unavailable"
    );
    let screen = text(&render(&mut app, 94, 29));
    assert!(!screen.contains("88767477"));
    assert!(!screen.contains("telemetry_unavailable"));
    assert!(screen.contains("Lamps ON failed: telemetry unavailable"));
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
    app.update(Event::Finished(
        "Done".into(),
        Feedback::new("AC ON confirmed", Severity::Success),
    ));
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
fn trends_use_timestamps_have_gap_columns_and_bounded_real_samples() {
    let mut app = app();
    app.samples.clear();
    let now = chrono::Utc::now();
    for (index, seconds) in [110, 100, 10, 0].into_iter().enumerate() {
        let mut current = status();
        let sample = current.telemetry.sample.as_mut().unwrap();
        sample.sequence = index as u64 + 1;
        sample.received_at = (now - chrono::Duration::seconds(seconds)).to_rfc3339();
        app.update(Event::Status(Box::new(current)));
    }
    let data = app.graph_data(40, false);
    assert_eq!(data.iter().filter(|&&v| v != 0).count(), 4);
    assert!(data[10..25].iter().all(|&v| v == 0));
    for sequence in 5..700 {
        let mut current = status();
        current.telemetry.sample.as_mut().unwrap().sequence = sequence;
        app.update(Event::Status(Box::new(current)));
    }
    assert_eq!(app.samples.len(), 512);
    assert!(status().valid());
    let mut invalid = status();
    invalid.telemetry.sample.as_mut().unwrap().battery_percent = 101;
    assert!(!invalid.valid());
}

#[test]
fn stream_errors_keep_command_details_for_logs_but_replace_transient_feedback() {
    let mut app = app();
    app.update(Event::Finished(
        "Do not replay. Outcome uncertain | command-id".into(),
        Feedback::new(
            "Command outcome uncertain; check station",
            Severity::Warning,
        ),
    ));
    app.update(Event::Disconnected("Disconnected".into()));
    app.update(Event::Notice("Logs unavailable".into()));
    assert!(app.notice.contains("Outcome uncertain"));
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
fn concurrent_log_page_does_not_erase_runtime_action_feedback() {
    let mut app = app();
    let Effect::Logs(request) = app.key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE)) else {
        panic!("Expected log request");
    };
    app.update(Event::Finished(
        "Runtime log level updated.".into(),
        Feedback::new("Log level changed to DEBUG", Severity::Info),
    ));
    let page = serde_json::from_value(json!({
        "schema_version":1,"items":[],"previous_cursor":null,"next_cursor":null,
        "has_more_before":false,"has_more_after":false,"source":"files","gap":false,"skipped_lines":0,
    })).unwrap();
    app.update(Event::LogPage(request, Ok(page)));
    assert!(text(&render(&mut app, 60, 19)).contains("Runtime log level updated."));
    app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(app.logs.action.is_none());
}
