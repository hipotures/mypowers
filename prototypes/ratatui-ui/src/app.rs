use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

const HISTORY: usize = 128;
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct App {
    pub live: bool,
    pub colored_bars: bool,
    pub tick: usize,
    pub input: u64,
    pub output: u64,
    pub input_history: Vec<u64>,
    pub output_history: Vec<u64>,
    pub enabled: [bool; 3],
    pub controls: [Rect; 3],
    pub hovered: Option<usize>,
    pub selected: Option<usize>,
    sample: u64,
}

impl Default for App {
    fn default() -> Self {
        let input_history = (0..HISTORY).map(|i| wave(i as f64, 100)).collect();
        let output_history = (0..HISTORY).map(|i| wave(i as f64 + 14.0, 300)).collect();
        let mut app = Self {
            live: true,
            colored_bars: true,
            tick: 0,
            input: 63,
            output: 181,
            input_history,
            output_history,
            enabled: [true, false, false],
            controls: [Rect::default(); 3],
            hovered: None,
            selected: None,
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

    pub fn mouse(&mut self, mouse: MouseEvent) {
        let hit = self
            .controls
            .iter()
            .position(|rect| rect.contains(Position::new(mouse.column, mouse.row)));
        self.hovered = hit;
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(index) = hit
        {
            self.toggle(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
