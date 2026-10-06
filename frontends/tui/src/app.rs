use crate::{
    clock::Clock,
    feedback::{Feedback, Severity, output_name},
    model::{Command, Status},
    network::{ClipboardTarget, Event, Intent},
    settings::{Field, Picker, Settings, SettingsTab},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
pub enum View {
    Dashboard,
    Logs,
    Help,
    Settings,
    Quit,
}

pub struct App {
    pub client_preferences: crate::client_ui::ClientPreferences,
    pub clock: Clock,
    pub snapshot: bool,
    pub warning_count: u32,
    pub error_count: u32,
    pub status: Option<Status>,
    pub connected: bool,
    pub server_url: String,
    pub received: Instant,
    pub animation_started: Instant,
    pub graph: crate::history::Graph,
    pub controls: [Rect; 3],
    pub hovered: Option<usize>,
    pub selected: Option<usize>,
    pub title: Rect,
    pub feedback: Option<Feedback>,
    pub connection_notice: String,
    pub log_notice: String,
    pub pending: Option<String>,
    pub settings_tab: SettingsTab,
    pub settings_tabs: [Rect; 5],
    pub settings: Option<Settings>,
    pub settings_draft: Settings,
    pub settings_selected: usize,
    pub settings_fields: Vec<(Rect, Field)>,
    pub settings_choices: Vec<(Rect, usize)>,
    pub settings_actions: [Rect; 3],
    pub settings_save: Rect,
    pub settings_picker: Option<Picker>,
    settings_press: Option<usize>,
    pub graph_base_scale_w: u64,
    graph_view_overridden: bool,
    timezone_overridden: bool,
    pub settings_error: bool,
    graph_interval_overridden: bool,
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
    history_generation: u64,
    history_requested: bool,
    history_retry_after: Option<Instant>,
    history_failed: bool,
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
        Self::with_clock(no_color, timezone, Clock::Live)
    }

    pub fn with_clock(no_color: bool, timezone: Option<chrono_tz::Tz>, clock: Clock) -> Self {
        Self {
            client_preferences: crate::client_ui::ClientPreferences::default(),
            clock,
            warning_count: 0,
            error_count: 0,
            snapshot: false,
            status: None,
            connected: false,
            server_url: "http://127.0.0.1:8765".into(),
            received: Instant::now(),
            animation_started: Instant::now(),
            graph: crate::history::Graph::default(),
            controls: [Rect::default(); 3],
            hovered: None,
            selected: Some(0),
            title: Rect::default(),
            feedback: Some(Feedback::new("Connecting to daemon...", Severity::Info)),
            connection_notice: "Connecting to daemon...".into(),
            log_notice: String::new(),
            pending: None,
            settings_tab: SettingsTab::default(),
            settings_tabs: [Rect::default(); 5],
            settings: None,
            settings_draft: Settings::default(),
            settings_selected: 0,
            settings_fields: Vec::new(),
            settings_choices: Vec::new(),
            settings_actions: [Rect::default(); 3],
            settings_save: Rect::default(),
            settings_picker: None,
            settings_press: None,
            graph_base_scale_w: 100,
            graph_view_overridden: false,
            timezone_overridden: timezone.is_some(),
            settings_error: false,
            graph_interval_overridden: false,
            view: View::Dashboard,
            help_context: View::Dashboard,
            logs: crate::logs::Logs::new_at(timezone, clock.now()),
            no_color,
            timezone,
            title_click: None,
            press: None,
            generation: 0,
            quit_yes: true,
            quit_buttons: [Rect::default(); 2],
            started_at: clock.now(),
            last_command: None,
            history_generation: 0,
            history_requested: false,
            history_retry_after: None,
            history_failed: false,
        }
    }

    pub fn update(&mut self, event: Event) {
        match event {
            Event::Settings(settings) => {
                if !settings.valid() {
                    return;
                }
                let saving = self.pending.as_deref() == Some("settings request");
                let first = self.settings.is_none();
                if first || saving {
                    self.apply_settings(&settings, saving);
                }
                if first || saving || self.settings.as_ref() == Some(&self.settings_draft) {
                    self.settings_draft = settings.clone();
                }
                self.settings = Some(settings);
                self.settings_error = false;
            }
            Event::SettingsUnavailable => {
                if self.connected && !self.settings_error {
                    self.feedback = Some(Feedback::new(
                        "Settings unavailable; retrying",
                        Severity::Warning,
                    ));
                }
                self.settings_error = true;
            }

            Event::LogStreamReady => self.log_notice.clear(),
            Event::Status(status) => {
                self.connection_feedback(&status);
                let changed = self
                    .status
                    .as_ref()
                    .is_some_and(|old| old.server_instance_id != status.server_instance_id);
                if changed {
                    self.graph.points.clear();
                    self.reset_history();
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
                if self.status.as_ref().is_some_and(|old| {
                    chrono::DateTime::parse_from_rfc3339(&status.server_time).ok()
                        < chrono::DateTime::parse_from_rfc3339(&old.server_time).ok()
                }) {
                    self.graph.points.clear();
                    self.reset_history();
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
                self.reset_history();
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
            Event::History(request, result) => self.accept_history(&request, result),
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

    fn reset_history(&mut self) {
        self.history_generation += 1;
        self.history_requested = false;
        self.history_retry_after = None;
        self.history_failed = false;
    }

    pub fn history_request(&mut self) -> Option<crate::history::Request> {
        if !self.connected
            || self.history_requested
            || self
                .history_retry_after
                .is_some_and(|deadline| Instant::now() < deadline)
        {
            return None;
        }
        let status = self.status.as_ref()?;
        if status.history["state"] != "ok" {
            return None;
        }
        let request = crate::history::Request::new(
            status,
            self.history_generation,
            self.graph.resolution,
            self.graph.width,
            self.timeline_now_ms(),
        )?;
        self.history_requested = true;
        Some(request)
    }

    fn accept_history(
        &mut self,
        request: &crate::history::Request,
        result: Result<Vec<crate::history::Point>, String>,
    ) {
        if request.generation != self.history_generation
            || !self.connected
            || self
                .status
                .as_ref()
                .is_none_or(|status| status.server_instance_id != request.server_instance_id)
        {
            return;
        }
        self.history_requested = false;
        match result {
            Ok(points) => {
                self.graph.points = points;
                self.graph.prune(self.timeline_now_ms());
                self.history_failed = false;
                self.history_retry_after = Some(Instant::now() + Duration::from_secs(5));
            }
            Err(_) => {
                if !self.history_failed
                    && self
                        .feedback
                        .as_ref()
                        .is_none_or(|feedback| feedback.started <= request.started)
                {
                    self.feedback = Some(Feedback::new(
                        "Graph history unavailable; current readings remain live",
                        Severity::Warning,
                    ));
                }
                self.history_failed = true;
                self.history_retry_after = Some(Instant::now() + Duration::from_secs(2));
            }
        }
    }

    pub fn set_graph_width(&mut self, width: u16) {
        let width = width.clamp(1, crate::history::MAX_BUCKETS);
        if self.graph.width != width {
            self.graph.width = width;
            self.reset_history();
        }
    }

    pub fn age(&self) -> Option<f64> {
        self.status
            .as_ref()?
            .telemetry
            .age_seconds
            .map(|age| age + self.clock.telemetry_elapsed(self.received).as_secs_f64())
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
        if self.view == View::Settings && self.settings_picker.is_some() {
            return self.picker_key(key);
        }
        let mode = match self.view {
            View::Dashboard => "dashboard",
            View::Logs => "logs",
            View::Settings => "settings",
            _ => "",
        };
        let key = self.client_preferences.key(mode, key);
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
                KeyCode::Char('s') => {
                    self.view = View::Settings;
                    self.resize();
                }
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
                KeyCode::Char('t') => self.cycle_graph_interval(),
                KeyCode::Char('g') => self.toggle_graph_view(),
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
            View::Settings => match key.code {
                KeyCode::F(1) | KeyCode::Char('?') => self.open_help(),
                KeyCode::Tab => self.select_settings_tab(self.settings_tab.next(false)),
                KeyCode::BackTab => self.select_settings_tab(self.settings_tab.next(true)),
                KeyCode::Up | KeyCode::Down => {
                    let count = self.settings_count();
                    if count > 0 {
                        self.settings_selected = (self.settings_selected
                            + if key.code == KeyCode::Up {
                                count - 1
                            } else {
                                1
                            })
                            % count;
                    }
                }
                KeyCode::Enter | KeyCode::Char(' ') => return self.edit_setting(),
                _ => {}
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

    fn apply_settings(&mut self, settings: &Settings, saving: bool) {
        if saving || !self.graph_interval_overridden {
            self.graph.resolution = settings.resolution().unwrap();
        }
        if saving || !self.graph_view_overridden {
            self.graph.visualization = settings.graph_visualization;
        }
        self.graph_base_scale_w = settings.graph_base_scale_w;
        if saving || !self.timezone_overridden {
            self.timezone = settings.timezone.parse().ok();
        }
        self.logs
            .configure(self.timezone, settings.logs_page_size, self.clock.now());
        self.graph.points.clear();
        self.resize();
    }

    fn settings_count(&self) -> usize {
        match self.settings_tab {
            SettingsTab::Preferences => 4,
            SettingsTab::Charts => 4,
            SettingsTab::Debug => 3,
            SettingsTab::Alerts => 5,
            SettingsTab::Notify => 1,
        }
    }

    fn edit_setting(&mut self) -> Effect {
        if self.settings_count() == 0 {
            return Effect::None;
        }
        if self.settings_tab == SettingsTab::Debug {
            return self.operation(['r', 'p', 'b'][self.settings_selected]);
        }
        if self.settings_tab == SettingsTab::Notify {
            if self.connected && self.pending.is_none() {
                self.pending = Some("telegram test".into());
                self.feedback = Some(Feedback::new("Sending Telegram test...", Severity::Info));
                return Effect::Request(Intent::TestTelegram);
            }
            return Effect::None;
        }
        if self.settings_tab == SettingsTab::Preferences && self.settings_selected == 2 {
            self.settings_picker = Some(Picker {
                field: Field::Theme,
                selected: crate::client_ui::THEMES
                    .iter()
                    .position(|name| *name == self.client_preferences.theme)
                    .unwrap_or(0),
                query: String::new(),
            });
            self.resize();
            return Effect::None;
        }
        if self.settings.is_none() || self.pending.is_some() {
            return Effect::None;
        }
        if self.settings_selected == self.settings_count() - 1 {
            return self.save_settings();
        }
        let field = match self.settings_tab {
            SettingsTab::Preferences => Field::PREFERENCES[self.settings_selected],
            SettingsTab::Charts => Field::CHARTS[self.settings_selected],
            SettingsTab::Alerts => Field::ALERTS[self.settings_selected],
            _ => return Effect::None,
        };
        let selected = field
            .choices()
            .iter()
            .position(|value| *value == self.settings_draft.value(field))
            .unwrap_or(0);
        self.settings_picker = Some(Picker {
            field,
            selected,
            query: String::new(),
        });
        self.resize();
        Effect::None
    }

    fn save_settings(&mut self) -> Effect {
        if self.connected && self.settings.is_some() && self.pending.is_none() {
            self.pending = Some("settings request".into());
            self.feedback = Some(Feedback::new("Saving settings...", Severity::Info));
            Effect::Request(Intent::SaveSettings(self.settings_draft.clone()))
        } else {
            self.feedback = Some(Feedback::new(
                "Settings unavailable or operation pending",
                Severity::Warning,
            ));
            Effect::None
        }
    }

    fn picker_key(&mut self, key: KeyEvent) -> Effect {
        if key.code == KeyCode::Char('q') {
            self.settings_picker = None;
            self.view = View::Quit;
            self.quit_yes = true;
            self.resize();
            return Effect::None;
        }
        let picker = self.settings_picker.as_mut().unwrap();
        let options = picker.options();
        match key.code {
            KeyCode::Esc => {
                self.settings_picker = None;
                self.resize();
            }
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End => {
                if !options.is_empty() {
                    let step = match key.code {
                        KeyCode::PageUp | KeyCode::PageDown => 8,
                        _ => 1,
                    };
                    picker.selected = match key.code {
                        KeyCode::Home => 0,
                        KeyCode::End => options.len() - 1,
                        KeyCode::Up | KeyCode::PageUp => picker.selected.saturating_sub(step),
                        _ => (picker.selected + step).min(options.len() - 1),
                    };
                }
            }
            KeyCode::Enter => {
                if let Some((index, _)) = options.get(picker.selected) {
                    if picker.field == Field::Theme {
                        let name = crate::client_ui::THEMES[*index];
                        match self.client_preferences.save_theme(name) {
                            Ok(()) => {
                                self.feedback = Some(Feedback::new(
                                    format!("Theme saved locally: {name}"),
                                    Severity::Info,
                                ));
                                self.settings_picker = None;
                                self.resize();
                            }
                            Err(_) => {
                                self.feedback = Some(Feedback::new(
                                    "Cannot save local client theme",
                                    Severity::Error,
                                ))
                            }
                        }
                        return Effect::None;
                    }
                    let mut candidate = self.settings_draft.clone();
                    candidate.choose(picker.field, *index);
                    if !candidate.valid() {
                        self.feedback = Some(Feedback::new(
                            "Threshold + hysteresis must be at most 100%",
                            Severity::Warning,
                        ));
                        return Effect::None;
                    }
                    self.settings_draft = candidate;
                    self.settings_picker = None;
                    self.resize();
                }
            }
            KeyCode::Char(character) if !character.is_control() && picker.query.len() < 128 => {
                picker.query.push(character);
                picker.selected = 0;
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.selected = 0;
            }
            _ => {}
        }
        Effect::None
    }

    fn cycle_graph_interval(&mut self) {
        self.graph_interval_overridden = true;
        self.graph.resolution = self.graph.resolution.next();
        self.graph.points.clear();
        self.reset_history();
        self.feedback = Some(Feedback::new(
            format!("Graph interval: {} per bar", self.graph.resolution.label()),
            Severity::Info,
        ));
    }

    fn toggle_graph_view(&mut self) {
        self.graph_view_overridden = true;
        self.graph.visualization = self.graph.visualization.next();
        self.resize();
        self.feedback = Some(Feedback::new(
            format!("Graph view: {}", self.graph.visualization.label()),
            Severity::Info,
        ));
    }

    fn select_settings_tab(&mut self, tab: SettingsTab) {
        self.settings_tab = tab;
        self.settings_selected = 0;
        self.settings_picker = None;
        self.resize();
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
        self.settings_tabs = [Rect::default(); 5];
        self.settings_fields.clear();
        self.settings_choices.clear();
        self.settings_actions = [Rect::default(); 3];
        self.settings_save = Rect::default();
        self.settings_press = None;
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
        if self.view == View::Settings {
            let position = Position::new(mouse.column, mouse.row);
            if let Some(picker) = &mut self.settings_picker {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && let Some((_, index)) = self
                        .settings_choices
                        .iter()
                        .find(|(rect, _)| rect.contains(position))
                {
                    picker.selected = picker
                        .options()
                        .iter()
                        .position(|(value, _)| value == index)
                        .unwrap_or(0);
                    return self.picker_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
                return Effect::None;
            }
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                if let Some(index) = self
                    .settings_tabs
                    .iter()
                    .position(|rect| rect.contains(position))
                {
                    self.select_settings_tab(SettingsTab::ALL[index]);
                } else if let Some(index) = self
                    .settings_fields
                    .iter()
                    .position(|(rect, _)| rect.contains(position))
                {
                    self.settings_selected = index;
                    return self.edit_setting();
                } else if self.settings_save.contains(position) {
                    self.settings_selected = self.settings_count() - 1;
                    self.settings_press = Some(3);
                } else if let Some(index) = self
                    .settings_actions
                    .iter()
                    .position(|rect| rect.contains(position))
                {
                    self.settings_selected = index;
                    self.settings_press = Some(index);
                }
            } else if mouse.kind == MouseEventKind::Up(MouseButton::Left)
                && let Some(index) = self.settings_press.take()
            {
                if index == 3 {
                    if self.settings_save.contains(position) {
                        return self.save_settings();
                    }
                } else if self.settings_actions[index].contains(position) {
                    if self.settings_tab == SettingsTab::Notify {
                        return self.edit_setting();
                    }
                    return self.operation(['r', 'p', 'b'][index]);
                }
            }
            return Effect::None;
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

    pub fn graph_data(&self, width: u16, output: bool) -> Vec<f64> {
        self.graph.data(self.timeline_now_ms(), width, output)
    }

    pub fn has_power_history(&self, output: bool) -> bool {
        self.graph_data(self.graph.width, output)
            .iter()
            .any(|&value| value > 0.0)
    }

    pub fn timeline_now_ms(&self) -> i64 {
        self.status
            .as_ref()
            .and_then(|status| chrono::DateTime::parse_from_rfc3339(&status.server_time).ok())
            .map(|time| {
                time.timestamp_millis()
                    + self.clock.telemetry_elapsed(self.received).as_millis() as i64
            })
            .unwrap_or(0)
    }

    pub fn prune_graph(&mut self) {
        self.graph.prune(self.timeline_now_ms());
    }
}
