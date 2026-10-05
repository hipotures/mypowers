use crate::{
    feedback::{Feedback, Severity, output_name},
    model::{Command, Status},
    network::{ClipboardTarget, Event, Intent},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq)]
pub enum View {
    Dashboard,
    Logs,
    Help,
    Quit,
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
    pub feedback: Option<Feedback>,
    pub connection_notice: String,
    pub log_notice: String,
    pub pending: Option<String>,
    pub view: View,
    pub help_context: View,
    pub logs: crate::logs::Logs,
    pub no_color: bool,
    pub timezone: Option<chrono_tz::Tz>,
    title_click: Option<(Instant, Position)>,
    press: Option<(usize, u64, String, u64)>,
    generation: u64,
    pub quit_yes: bool,
    pub quit_buttons: [Rect; 2],
    started_at: chrono::DateTime<chrono::Utc>,
    last_command: Option<(String, String)>,
}

pub enum Effect {
    None,
    Quit,
    Copy,
    CopyLogs,
    Request(Intent),
    Logs(crate::logs::Request),
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
            feedback: Some(Feedback::new("Connecting to daemon...", Severity::Info)),
            connection_notice: "Connecting to daemon...".into(),
            log_notice: String::new(),
            pending: None,
            view: View::Dashboard,
            help_context: View::Dashboard,
            logs: crate::logs::Logs::new(timezone),
            no_color,
            timezone,
            title_click: None,
            press: None,
            generation: 0,
            quit_yes: true,
            quit_buttons: [Rect::default(); 2],
            started_at: chrono::Utc::now(),
            last_command: None,
        }
    }

    pub fn update(&mut self, event: Event) {
        match event {
            Event::Status(status) => {
                self.connection_feedback(&status);
                let changed = self
                    .status
                    .as_ref()
                    .is_some_and(|old| old.server_instance_id != status.server_instance_id);
                if changed {
                    self.samples.clear();
                    self.press = None;
                    self.last_command = None;
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
                if self.connected || self.connection_notice == "Connecting to daemon..." {
                    self.feedback = Some(Feedback::new(
                        if reason.contains("authentication") || reason.contains("TLS") {
                            "Daemon unavailable; check authentication, address and TLS"
                        } else {
                            "Connection lost; reconnecting to daemon"
                        },
                        if reason.contains("authentication") || reason.contains("TLS") {
                            Severity::Error
                        } else {
                            Severity::Warning
                        },
                    ));
                }
                self.connected = false;
                self.press = None;
                self.connection_notice = reason;
            }
            Event::Command(command) => {
                if self.pending.is_none() {
                    self.show_command(&command);
                }
            }
            Event::Notice(message) => {
                if self.connected && self.log_notice != message {
                    self.feedback = Some(Feedback::new(
                        if message.contains("history gap") {
                            "Log history incomplete; see Logs"
                        } else {
                            "Log stream unavailable; reconnecting"
                        },
                        Severity::Warning,
                    ));
                }
                self.log_notice = message;
            }
            Event::Finished(feedback) => {
                self.pending = None;
                self.feedback = Some(feedback);
            }
            Event::Log(record) => {
                let recent = record["timestamp"]
                    .as_str()
                    .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
                    .is_some_and(|time| time >= self.started_at);
                let feedback = recent.then(|| Feedback::log(&record)).flatten();
                if self.logs.record(record)
                    && let Some(feedback) = feedback
                {
                    self.feedback = Some(feedback);
                }
            }
            Event::LogPage(request, page) => {
                let current = request.generation == self.logs.generation();
                let failed = page.is_err();
                let feedback = if page.is_ok() {
                    Feedback::new("Logs loaded", Severity::Info)
                } else {
                    Feedback::new("Could not load logs; see Logs", Severity::Error)
                };
                self.logs.accept(&request, page);
                if current
                    && (failed
                        || self
                            .feedback
                            .as_ref()
                            .is_none_or(|feedback| feedback.started <= request.started))
                {
                    self.feedback = Some(feedback);
                }
            }
            Event::Copied(success, target) => {
                self.feedback = Some(Feedback::new(
                    match (success, target) {
                        (true, ClipboardTarget::Logs) => "Logs copied",
                        (true, ClipboardTarget::Snapshot) => "JSON copied",
                        (false, _) => "Copy failed; check clipboard access",
                    },
                    if success {
                        Severity::Success
                    } else {
                        Severity::Error
                    },
                ));
            }
            Event::Exit => {}
        }
    }

    pub fn show_command(&mut self, command: &Command) {
        let identity = (command.command_id.clone(), command.status.clone());
        if self.last_command.as_ref() == Some(&identity) {
            return;
        }
        self.last_command = Some(identity);
        self.feedback = Some(Feedback::command(command));
    }

    pub fn age(&self) -> Option<f64> {
        self.status
            .as_ref()?
            .telemetry
            .age_seconds
            .map(|age| age + self.received.elapsed().as_secs_f64())
    }

    fn connection_feedback(&mut self, status: &Status) {
        let old = self.status.as_ref();
        let new_instance =
            old.is_some_and(|old| old.server_instance_id != status.server_instance_id);
        let phase_changed = old.is_none_or(|old| old.connection.phase != status.connection.phase);
        if !self.connected || new_instance || phase_changed {
            let (message, severity) = match status.connection.phase.as_str() {
                "connected" => (
                    if old.is_some() {
                        "Connection restored"
                    } else {
                        "Connected to station"
                    },
                    Severity::Success,
                ),
                "reconnecting" | "backoff" => ("Reconnecting to station", Severity::Warning),
                "scanning" => ("Searching for station", Severity::Info),
                "connecting" => ("Connecting to station", Severity::Info),
                "paused" => ("Station connection paused", Severity::Info),
                "waiting_for_telemetry" => ("Waiting for station data", Severity::Info),
                _ => ("Waiting for station connection", Severity::Info),
            };
            self.feedback = Some(Feedback::new(message, severity));
        } else if old.is_some_and(|old| old.telemetry.state != status.telemetry.state) {
            let (message, severity) = if status.telemetry.state == "live" {
                ("Station data restored", Severity::Success)
            } else {
                ("Station data unavailable", Severity::Warning)
            };
            self.feedback = Some(Feedback::new(message, severity));
        } else if old
            .is_some_and(|old| old.logging["effective_level"] != status.logging["effective_level"])
            && let Some(level) = status.logging["effective_level"].as_str()
            && ["DEBUG", "INFO", "WARNING", "ERROR"].contains(&level)
        {
            self.feedback = Some(Feedback::new(
                format!("Log level changed to {level}"),
                Severity::Info,
            ));
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
            self.feedback = Some(Feedback::new(
                if !self.live() {
                    "Controls unavailable: fresh station data required"
                } else {
                    "Controls unavailable: another command is pending"
                },
                Severity::Warning,
            ));
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
        self.feedback = Some(Feedback::new(
            format!(
                "Waiting for {} {} confirmation...",
                output_name(output),
                if enabled { "ON" } else { "OFF" }
            ),
            Severity::Info,
        ));
        Effect::Request(Intent::Output {
            output,
            enabled,
            snapshot: Box::new(snapshot),
            key: uuid::Uuid::new_v4().to_string(),
        })
    }

    fn operation(&mut self, key: char) -> Effect {
        if !self.connected || self.pending.is_some() {
            self.feedback = Some(Feedback::new(
                "Daemon unavailable or operation pending",
                Severity::Warning,
            ));
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
        self.feedback = Some(Feedback::new("Sending request...", Severity::Info));
        Effect::Request(intent)
    }

    pub fn key(&mut self, key: KeyEvent) -> Effect {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            return Effect::Quit;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Effect::None;
        }
        if key.code == KeyCode::Esc {
            if self.view != View::Dashboard {
                self.view = View::Dashboard;
                self.resize();
            }
            return Effect::None;
        }
        if key.code == KeyCode::Char('q') {
            self.view = View::Quit;
            self.quit_yes = true;
            self.resize();
            return Effect::None;
        }
        match self.view {
            View::Dashboard => match key.code {
                KeyCode::F(3) => {
                    self.view = View::Logs;
                    self.resize();
                    if let Some(request) = self.logs.open() {
                        return Effect::Logs(request);
                    }
                }
                KeyCode::F(1) | KeyCode::Char('?') => self.open_help(),
                KeyCode::Char('a' | 'd' | 'l') => {
                    return self.toggle(match key.code {
                        KeyCode::Char('a') => 0,
                        KeyCode::Char('d') => 1,
                        _ => 2,
                    });
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    let step = if key.code == KeyCode::Tab { 1 } else { 2 };
                    self.selected = Some((self.selected.unwrap_or(0) + step) % 3);
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    return self.toggle(self.selected.unwrap_or(0));
                }
                KeyCode::Char(key @ ('r' | 'p')) => return self.operation(key),
                _ => {}
            },
            View::Logs => match key.code {
                KeyCode::F(1) | KeyCode::Char('?') => self.open_help(),
                KeyCode::Char('b') => return self.operation('b'),
                _ => {
                    if let Some(request) = self.logs.key(key.code) {
                        return Effect::Logs(request);
                    }
                }
            },
            View::Help => {}
            View::Quit => match key.code {
                KeyCode::Enter | KeyCode::Char('y')
                    if self.quit_yes || key.code == KeyCode::Char('y') =>
                {
                    return Effect::Quit;
                }
                KeyCode::Enter | KeyCode::Char('n') => {
                    self.view = View::Dashboard;
                    self.resize();
                }
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                    self.quit_yes = !self.quit_yes;
                }
                _ => {}
            },
        }
        Effect::None
    }

    fn open_help(&mut self) {
        self.help_context = self.view;
        self.view = View::Help;
        self.resize();
    }

    pub fn resize(&mut self) {
        self.hovered = None;
        self.controls = [Rect::default(); 3];
        self.title = Rect::default();
        self.quit_buttons = [Rect::default(); 2];
        self.title_click = None;
        self.press = None;
        self.logs.resize();
        self.generation += 1;
    }

    pub fn mouse(&mut self, mouse: MouseEvent) -> Effect {
        if self.view == View::Quit {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                let position = Position::new(mouse.column, mouse.row);
                if self.quit_buttons[0].contains(position) {
                    return Effect::Quit;
                }
                if self.quit_buttons[1].contains(position) {
                    self.view = View::Dashboard;
                    self.resize();
                }
            }
            return Effect::None;
        }
        if self.view == View::Logs {
            if self.logs.copy_click(mouse) {
                return Effect::CopyLogs;
            }
            return self
                .logs
                .mouse(mouse)
                .map(Effect::Logs)
                .unwrap_or(Effect::None);
        }
        if self.view != View::Dashboard {
            return Effect::None;
        }
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
