use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

const HISTORY: usize = 128;

pub struct App {
    pub live: bool,
    pub colored_bars: bool,
    pub input: u64,
    pub output: u64,
    pub input_history: Vec<u64>,
    pub output_history: Vec<u64>,
    pub enabled: [bool; 3],
    pub controls: [Rect; 3],
    pub hovered: Option<usize>,
    pub selected: Option<usize>,
    pub title: Rect,
    pub clipboard_notice: Option<(bool, Instant)>,
    last_title_click: Option<(Instant, Position)>,
    sample: u64,
}

impl Default for App {
    fn default() -> Self {
        let input_history = (0..HISTORY).map(|i| wave(i as f64, 100)).collect();
        let output_history = (0..HISTORY).map(|i| wave(i as f64 + 14.0, 300)).collect();
        let mut app = Self {
            live: true,
            colored_bars: true,
            input: 63,
            output: 181,
            input_history,
            output_history,
            enabled: [true, false, false],
            controls: [Rect::default(); 3],
            hovered: None,
            selected: None,
            title: Rect::default(),
            clipboard_notice: None,
            last_title_click: None,
            sample: 0,
        };
        app.input_history[HISTORY - 1] = app.input;
        app.output_history[HISTORY - 1] = app.output;
        app
    }
}

fn wave(t: f64, maximum: u64) -> u64 {
    ((0.5 + 0.30 * (t / 7.5).sin() + 0.09 * (t / 2.9).cos()) * maximum as f64).round() as u64
}

impl App {
    pub fn sample(&mut self) {
        self.sample += 1;
        let t = self.sample as f64;
        self.input = (63.0 + 11.0 * (t / 9.0).sin() + 4.0 * (t / 3.0).sin()).round() as u64;
        self.output = (181.0 + 29.0 * (t / 11.0).sin() + 9.0 * (t / 4.0).sin()).round() as u64;
        self.input_history.rotate_left(1);
        self.output_history.rotate_left(1);
        *self.input_history.last_mut().unwrap() = self.input;
        *self.output_history.last_mut().unwrap() = self.output;
    }

    pub fn key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Char('a') => self.toggle(0),
            KeyCode::Char('d') => self.toggle(1),
            KeyCode::Char('l') => self.toggle(2),
            _ => {}
        }
        false
    }

    fn toggle(&mut self, index: usize) {
        self.enabled[index] = !self.enabled[index];
        self.selected = Some(index);
    }

    pub fn mock_json(&self) -> String {
        format!(
            "{{\n  \"mock\": true,\n  \"station\": \"AP S300 V2.0\",\n  \"connected\": {},\n  \"battery_percent\": 78,\n  \"remaining_minutes\": 2937,\n  \"input_w\": {},\n  \"output_w\": {},\n  \"outputs\": {{\n    \"ac\": {},\n    \"dc\": {},\n    \"lamps\": {}\n  }}\n}}\n",
            self.live, self.input, self.output, self.enabled[0], self.enabled[1], self.enabled[2],
        )
    }

    pub fn resize(&mut self) {
        self.hovered = None;
        self.controls = [Rect::default(); 3];
        self.title = Rect::default();
        self.last_title_click = None;
    }

    pub fn mouse(&mut self, mouse: MouseEvent) -> bool {
        self.mouse_at(mouse, Instant::now())
    }

    fn mouse_at(&mut self, mouse: MouseEvent, now: Instant) -> bool {
        let position = Position::new(mouse.column, mouse.row);
        let hit = self
            .controls
            .iter()
            .position(|rect| rect.contains(position));
        self.hovered = hit;
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            let previous = self.last_title_click.take();
            if self.title.contains(position) {
                if let Some((time, previous_position)) = previous
                    && now.duration_since(time) <= Duration::from_millis(400)
                    && previous_position.y == position.y
                    && previous_position.x.abs_diff(position.x) <= 1
                {
                    return true;
                }
                self.last_title_click = Some((now, position));
            } else if let Some(index) = hit {
                self.toggle(index);
            }
        } else if matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.last_title_click = None;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_copy_requires_two_nearby_left_clicks_within_400_ms() {
        let mut app = App {
            title: Rect::new(10, 0, 8, 1),
            ..App::default()
        };
        let click = |x, row, button| MouseEvent {
            kind: MouseEventKind::Down(button),
            column: x,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let now = Instant::now();
        assert!(!app.mouse_at(click(12, 0, MouseButton::Left), now));
        assert!(app.mouse_at(
            click(13, 0, MouseButton::Left),
            now + Duration::from_millis(200)
        ));
        assert!(!app.mouse_at(
            click(12, 0, MouseButton::Left),
            now + Duration::from_secs(1)
        ));
        assert!(!app.mouse_at(
            click(12, 0, MouseButton::Left),
            now + Duration::from_secs(2)
        ));
        assert!(!app.mouse_at(
            click(16, 0, MouseButton::Left),
            now + Duration::from_millis(2200)
        ));
        assert!(!app.mouse_at(
            click(16, 0, MouseButton::Right),
            now + Duration::from_millis(2250)
        ));
        assert!(!app.mouse_at(
            click(16, 0, MouseButton::Left),
            now + Duration::from_millis(2300)
        ));
        assert!(!app.mouse_at(
            click(16, 1, MouseButton::Left),
            now + Duration::from_millis(2350)
        ));
        assert!(!app.mouse_at(
            click(16, 0, MouseButton::Left),
            now + Duration::from_millis(2400)
        ));
        app.resize();
        app.title = Rect::new(10, 0, 8, 1);
        assert!(!app.mouse_at(
            click(16, 0, MouseButton::Left),
            now + Duration::from_millis(2450)
        ));
        assert_eq!(app.enabled, [true, false, false]);
    }

    #[test]
    fn mock_json_reflects_current_telemetry_and_local_outputs() {
        let app = App {
            live: false,
            input: 55,
            output: 191,
            enabled: [false, true, true],
            ..App::default()
        };
        let json = app.mock_json();
        for expected in [
            "\"mock\": true",
            "\"station\": \"AP S300 V2.0\"",
            "\"connected\": false",
            "\"battery_percent\": 78",
            "\"remaining_minutes\": 2937",
            "\"input_w\": 55",
            "\"output_w\": 191",
            "\"ac\": false",
            "\"dc\": true",
            "\"lamps\": true",
        ] {
            assert!(json.contains(expected));
        }
    }

    #[test]
    fn telemetry_shifts_history_slowly_with_bounded_fake_values() {
        let mut app = App::default();
        let previous = app.input_history[1];
        app.sample();
        assert_eq!(app.input_history[0], previous);
        for _ in 0..1000 {
            app.sample();
            assert!(app.input <= 100 && app.output <= 300);
            assert_eq!(app.input_history.len(), HISTORY);
            assert_eq!(app.output_history.len(), HISTORY);
            assert_eq!(app.input_history.last(), Some(&app.input));
            assert_eq!(app.output_history.last(), Some(&app.output));
        }
    }

    #[test]
    fn keyboard_and_mouse_only_change_local_control_states() {
        let mut app = App::default();
        for key in ['a', 'd', 'l'] {
            assert!(!app.key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)));
        }
        assert_eq!(app.enabled, [false, true, true]);
        assert_eq!((app.input, app.output), (63, 181));
        app.controls = [
            Rect::new(1, 1, 10, 2),
            Rect::new(11, 1, 10, 2),
            Rect::new(21, 1, 10, 2),
        ];
        app.mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 12,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.hovered, Some(1));
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 12,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.enabled, [false, false, true]);
        assert_eq!(app.selected, Some(1));
        app.mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 55,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.hovered, None);
        assert!(app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)));
        assert!(app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
    }
}
