use crate::{
    model::{Command, Status, safe},
    network::{Event, Intent},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use serde_json::Value;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq)]
pub enum View {
    Dashboard,
    Logs,
    Help,
}

pub struct Trend {
    pub timestamp: f64,
    pub sample: crate::model::Sample,
}

pub struct App {
    pub status: Option<Status>,
    pub connected: bool,
    pub received: Instant,
    pub samples: VecDeque<Trend>,
    pub controls: [Rect; 3],
    pub hovered: Option<usize>,
    pub selected: Option<usize>,
    pub title: Rect,
    pub clipboard_notice: Option<(bool, Instant)>,
    pub notice: String,
    pub connection_notice: String,
    pub log_notice: String,
    pub pending: Option<String>,
    pub view: View,
    pub logs: VecDeque<Value>,
    pub log_level: usize,
    pub scroll: usize,
    pub no_color: bool,
    pub timezone: Option<chrono_tz::Tz>,
    title_click: Option<(Instant, Position)>,
    press: Option<(usize, u64, String, u64)>,
    generation: u64,
}

pub enum Effect {
    None,
    Quit,
    Copy,
    Request(Intent),
}

impl App {
    pub fn new(no_color: bool, timezone: Option<chrono_tz::Tz>) -> Self {
        Self {
            status: None,
            connected: false,
            received: Instant::now(),
            samples: VecDeque::new(),
            controls: [Rect::default(); 3],
            hovered: None,
            selected: Some(0),
            title: Rect::default(),
            clipboard_notice: None,
            notice: String::new(),
            connection_notice: "Connecting to daemon...".into(),
            log_notice: String::new(),
            pending: None,
            view: View::Dashboard,
            logs: VecDeque::new(),
            log_level: 0,
            scroll: 0,
            no_color,
            timezone,
            title_click: None,
            press: None,
            generation: 0,
        }
    }

    pub fn update(&mut self, event: Event) {
        match event {
            Event::Status(status) => {
                let changed = self
                    .status
                    .as_ref()
                    .is_some_and(|old| old.server_instance_id != status.server_instance_id);
                if changed {
                    self.samples.clear();
                    self.press = None;
                }
                if self
                    .status
                    .as_ref()
                    .is_none_or(|old| old.connection.session_id != status.connection.session_id)
                {
                    self.press = None;
                }
                if status.telemetry.state == "live"
                    && let Some(sample) = &status.telemetry.sample
                {
                    let duplicate = self.samples.back().is_some_and(|old| {
                        old.sample.sequence == sample.sequence
                            && old.sample.segment_id == sample.segment_id
                    });
                    if !duplicate {
                        let timestamp = chrono::DateTime::parse_from_rfc3339(&sample.received_at)
                            .unwrap()
                            .timestamp_millis() as f64
                            / 1000.0;
                        if self
                            .samples
                            .back()
                            .is_some_and(|old| timestamp < old.timestamp)
                        {
                            self.samples.clear();
                        }
                        self.samples.push_back(Trend {
                            timestamp,
                            sample: sample.clone(),
                        });
                        while self.samples.len() > 512
                            || self
                                .samples
                                .front()
                                .is_some_and(|old| timestamp - old.timestamp > 120.0)
                        {
                            self.samples.pop_front();
                        }
                    }
                }
                self.connection_notice = "Connected to daemon.".into();
                self.received = Instant::now();
                self.connected = true;
                self.status = Some(*status);
            }
            Event::Disconnected(reason) => {
                self.connected = false;
                self.press = None;
                self.connection_notice = reason;
            }
            Event::Command(command) => {
                if self.pending.is_none() {
                    self.show_command(&command);
                }
            }
            Event::Notice(message) => self.log_notice = message,
            Event::Finished(message) => {
                self.pending = None;
                self.notice = message;
            }
            Event::Log(record) => {
                if record.get("sequence").is_some()
                    && self.logs.iter().any(|old| {
                        old["sequence"] == record["sequence"]
                            && old["server_instance_id"] == record["server_instance_id"]
                    })
                {
                    return;
                }
                self.logs.push_back(record);
                while self.logs.len() > 1000 {
                    self.logs.pop_front();
                }
            }
            Event::Copied(success) => self.clipboard_notice = Some((success, Instant::now())),
            Event::Exit => {}
        }
    }

    pub fn show_command(&mut self, command: &Command) {
        self.notice = format!(
            "{} {} | {}",
            command.output.to_uppercase(),
            command.status,
            command.command_id
        );
        if let Some(reason) = &command.reason_code {
            self.notice.push_str(&format!(" | {}", safe(reason)));
        }
    }

    pub fn age(&self) -> Option<f64> {
        self.status
            .as_ref()?
            .telemetry
            .age_seconds
            .map(|age| age + self.received.elapsed().as_secs_f64())
    }

    pub fn display_notice(&self) -> &str {
        if !self.notice.is_empty() {
            &self.notice
        } else if self.view == View::Logs && !self.log_notice.is_empty() {
            &self.log_notice
        } else {
            &self.connection_notice
        }
    }

    pub fn live(&self) -> bool {
        self.connected
            && self
                .status
                .as_ref()
                .is_some_and(|s| s.connection.link_connected && s.telemetry.state == "live")
            && self.age().is_some_and(|age| age < 3.0)
    }

    pub fn allowed(&self) -> bool {
        self.live()
            && self.pending.is_none()
            && self
                .status
                .as_ref()
                .is_some_and(|s| s.controls.allowed && s.telemetry.sample.is_some())
    }

    pub fn toggle(&mut self, index: usize) -> Effect {
        self.selected = Some(index);
        if !self.allowed() {
            self.notice =
                "Controls unavailable: fresh data and no pending command required.".into();
            return Effect::None;
        }
        let snapshot = self.status.as_ref().unwrap().clone();
        let sample = snapshot.telemetry.sample.as_ref().unwrap();
        let enabled = ![sample.ac_enabled, sample.dc_enabled, sample.light_enabled][index];
        let output = ["ac", "dc", "light"][index];
        self.pending = Some(format!(
            "{} -> {}",
            output.to_uppercase(),
            if enabled { "ON" } else { "OFF" }
        ));
        self.notice = format!("Pending {}", self.pending.as_ref().unwrap());
        Effect::Request(Intent::Output {
            output,
            enabled,
            snapshot: Box::new(snapshot),
            key: uuid::Uuid::new_v4().to_string(),
        })
    }

    fn operation(&mut self, key: char) -> Effect {
        if !self.connected || self.pending.is_some() {
            self.notice = "Daemon unavailable or operation pending.".into();
            return Effect::None;
        }
        let status = self.status.as_ref().unwrap();
        let intent = match key {
            'r' => Intent::Retry,
            'p' => Intent::Connection(status.connection.desired == "paused"),
            'b' => Intent::Debug(status.logging["effective_level"] != "DEBUG"),
            _ => unreachable!(),
        };
        self.pending = Some("daemon request".into());
        Effect::Request(intent)
    }

    pub fn key(&mut self, key: KeyEvent) -> Effect {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'z'))
        {
            return Effect::Quit;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Effect::Quit,
            KeyCode::F(1) | KeyCode::Char('?') => {
                self.view = View::Help;
                self.resize();
            }
            KeyCode::F(2) => {
                self.view = View::Dashboard;
                self.resize();
            }
            KeyCode::F(3) => {
                self.view = View::Logs;
                self.resize();
            }
            KeyCode::Char('a' | 'd' | 'l') if self.view == View::Dashboard => {
                return self.toggle(match key.code {
                    KeyCode::Char('a') => 0,
                    KeyCode::Char('d') => 1,
                    _ => 2,
                });
            }
            KeyCode::Tab | KeyCode::BackTab if self.view == View::Dashboard => {
                let step = if key.code == KeyCode::Tab { 1 } else { 2 };
                self.selected = Some((self.selected.unwrap_or(0) + step) % 3);
            }
            KeyCode::Enter | KeyCode::Char(' ') if self.view == View::Dashboard => {
                return self.toggle(self.selected.unwrap_or(0));
            }
            KeyCode::Char(key @ ('r' | 'p' | 'b')) => return self.operation(key),
            KeyCode::Char('f') if self.view == View::Logs => {
                self.log_level = (self.log_level + 1) % 4;
                self.scroll = 0;
            }
            KeyCode::Up | KeyCode::PageUp if self.view == View::Logs => {
                self.scroll = (self.scroll + if key.code == KeyCode::Up { 1 } else { 10 }).min(1000)
            }
            KeyCode::Down | KeyCode::PageDown if self.view == View::Logs => {
                self.scroll =
                    self.scroll
                        .saturating_sub(if key.code == KeyCode::Down { 1 } else { 10 })
            }
            KeyCode::End if self.view == View::Logs => self.scroll = 0,
            _ => {}
        }
        Effect::None
    }

    pub fn resize(&mut self) {
        self.hovered = None;
        self.controls = [Rect::default(); 3];
        self.title = Rect::default();
        self.title_click = None;
        self.press = None;
        self.generation += 1;
    }

    pub fn mouse(&mut self, mouse: MouseEvent) -> Effect {
        let position = Position::new(mouse.column, mouse.row);
        let hit = self
            .controls
            .iter()
            .position(|rect| rect.contains(position));
        self.hovered = hit;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.press = None;
                let previous = self.title_click.take();
                if self.title.contains(position) {
                    if let Some((time, old)) = previous
                        && time.elapsed() <= Duration::from_millis(400)
                        && old.y == position.y
                        && old.x.abs_diff(position.x) <= 1
                    {
                        return Effect::Copy;
                    }
                    self.title_click = Some((Instant::now(), position));
                } else if let Some(index) = hit
                    && self.allowed()
                {
                    let status = self.status.as_ref().unwrap();
                    self.press = Some((
                        index,
                        self.generation,
                        status.server_instance_id.clone(),
                        status.controls.outputs_revision,
                    ));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some((index, generation, instance, revision)) = self.press.take()
                    && hit == Some(index)
                    && generation == self.generation
                    && self.status.as_ref().is_some_and(|s| {
                        s.server_instance_id == instance && s.controls.outputs_revision == revision
                    })
                {
                    return self.toggle(index);
                }
            }
            MouseEventKind::Down(_) => {
                self.press = None;
                self.title_click = None;
            }
            MouseEventKind::ScrollUp if self.view == View::Logs => {
                self.scroll = (self.scroll + 3).min(1000)
            }
            MouseEventKind::ScrollDown if self.view == View::Logs => {
                self.scroll = self.scroll.saturating_sub(3)
            }
            _ => {}
        }
        Effect::None
    }

    pub fn graph_data(&self, width: u16, output: bool) -> Vec<u64> {
        let mut data = vec![0; width as usize];
        if width == 0 {
            return data;
        }
        let now = self.timeline_now();
        let mut previous: Option<(&str, usize)> = None;
        for trend in &self.samples {
            let offset = trend.timestamp - (now - 120.0);
            if !(0.0..=120.0).contains(&offset) {
                continue;
            }
            let column = ((offset / 120.0 * f64::from(width)) as usize).min(width as usize - 1);
            if let Some((segment, old_column)) = previous
                && segment != trend.sample.segment_id
            {
                data[old_column] = 0;
            }
            data[column] = if output {
                trend.sample.output_power_w
            } else {
                trend.sample.input_power_w
            };
            previous = Some((&trend.sample.segment_id, column));
        }
        data
    }

    fn timeline_now(&self) -> f64 {
        self.status
            .as_ref()
            .and_then(|status| chrono::DateTime::parse_from_rfc3339(&status.server_time).ok())
            .map(|time| {
                time.timestamp_millis() as f64 / 1000.0 + self.received.elapsed().as_secs_f64()
            })
            .unwrap_or(0.0)
    }

    pub fn prune_trends(&mut self) {
        let cutoff = self.timeline_now() - 120.0;
        while self
            .samples
            .front()
            .is_some_and(|trend| trend.timestamp < cutoff)
        {
            self.samples.pop_front();
        }
    }
}
