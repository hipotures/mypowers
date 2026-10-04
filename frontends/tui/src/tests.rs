use crate::{
    app::{App, Effect, View},
    model::Status,
    network::{Event, Intent},
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
    app.update(Event::Finished("Outcome uncertain".into()));
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
    for (width, height) in [(60, 18), (80, 24), (94, 24), (120, 40)] {
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
    render(&mut unknown, 59, 18);
    assert!(unknown.title.is_empty());
}

#[test]
fn log_scrollbar_tracks_overflow_scroll_filter_and_resize() {
    let mut app = app();
    app.view = View::Logs;
    for sequence in 0..80 {
        app.update(Event::Log(json!({
            "sequence": sequence,
            "level": "INFO",
            "message": format!("Record {sequence:03}"),
        })));
    }
    let bottom = render(&mut app, 60, 18);
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
    assert!(!text(&render(&mut app, 60, 18)).contains("Record 079"));
    for _ in 0..100 {
        app.key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    }
    let top = render(&mut app, 60, 18);
    assert!(text(&top).contains("Record 000"));
    assert!(!text(&top).contains("Record 079"));
    assert!(thumb_rows(&top)[0] < thumb_rows(&bottom)[0]);
    app.key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert!(text(&render(&mut app, 60, 18)).contains("Record 079"));
    app.scroll = 1000;
    render(&mut app, 120, 40);
    assert_eq!(app.scroll, 80 - 35);
    app.log_level = 3;
    let filtered = render(&mut app, 60, 18);
    assert!(thumb_rows(&filtered).is_empty());
    assert_eq!(app.scroll, 0);
    app.log_level = 0;
    app.logs.truncate(4);
    assert!(thumb_rows(&render(&mut app, 60, 18)).is_empty());
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
    app.update(Event::Finished("Done".into()));
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
        Effect::Quit
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
fn stream_errors_do_not_erase_user_command_feedback() {
    let mut app = app();
    app.update(Event::Finished(
        "Do not replay. Outcome uncertain | command-id".into(),
    ));
    app.update(Event::Disconnected("Disconnected".into()));
    app.update(Event::Notice("Logs unavailable".into()));
    assert!(app.display_notice().contains("Outcome uncertain"));
    assert!(!app.connected);
}
