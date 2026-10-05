use chrono::{DateTime, Days, Local, NaiveDate, TimeZone, Utc};
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use serde::Deserialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const LEVELS: [&str; 4] = ["DEBUG", "INFO", "WARNING", "ERROR"];
const PAGE_SIZES: [usize; 5] = [50, 100, 250, 500, 1000];
const MAX_CACHED_PAGES: usize = 5;
const RECENT_LIMIT: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Load {
    Latest,
    Oldest,
    Older,
    Newer,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub started: Instant,
    pub generation: u64,
    pub kind: Load,
    pub since: String,
    pub until: String,
    pub level: &'static str,
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Page {
    pub schema_version: u8,
    pub items: Vec<Value>,
    pub previous_cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub has_more_before: bool,
    pub has_more_after: bool,
    pub source: String,
    pub gap: bool,
    pub skipped_lines: usize,
}

pub struct Logs {
    pub day: NaiveDate,
    pub level: usize,
    pub page_size: usize,
    pub records: VecDeque<Value>,
    pub offset: usize,
    pub follow: bool,
    pub loading: bool,
    pub message: String,
    pub unseen: usize,
    pub viewport: usize,
    pub scrollbar: Rect,
    pub thumb: Rect,
    pub buttons: [Rect; 2],
    pub title: Rect,
    pub more_before: bool,
    pub more_after: bool,
    timezone: Option<chrono_tz::Tz>,
    recent: VecDeque<Value>,
    previous_cursor: Option<String>,
    next_cursor: Option<String>,
    generation: u64,
    initialized: bool,
    drag: Option<u16>,
    pages: VecDeque<CachedPage>,
    title_click: Option<(Instant, Position)>,
}

struct CachedPage {
    count: usize,
    before: Option<String>,
    after: Option<String>,
}

impl Logs {
    pub fn new(timezone: Option<chrono_tz::Tz>) -> Self {
        Self::new_at(timezone, Utc::now())
    }

    pub fn new_at(timezone: Option<chrono_tz::Tz>, now: DateTime<Utc>) -> Self {
        Self {
            day: local_date(now, timezone),
            level: 0,
            page_size: 100,
            records: VecDeque::new(),
            offset: 0,
            follow: false,
            loading: false,
            message: String::new(),
            unseen: 0,
            viewport: 0,
            scrollbar: Rect::default(),
            thumb: Rect::default(),
            buttons: [Rect::default(); 2],
            title: Rect::default(),
            more_before: false,
            more_after: false,
            timezone,
            recent: VecDeque::new(),
            previous_cursor: None,
            next_cursor: None,
            generation: 0,
            initialized: false,
            drag: None,
            pages: VecDeque::new(),
            title_click: None,
        }
    }

    pub fn today(&self) -> NaiveDate {
        local_date(Utc::now(), self.timezone)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn zone_name(&self) -> String {
        self.timezone
            .map(|zone| zone.name().to_owned())
            .unwrap_or_else(|| Local::now().format("%Z").to_string())
    }

    pub fn open(&mut self) -> Option<Request> {
        if self.initialized {
            None
        } else {
            self.load(Load::Oldest)
        }
    }

    fn load(&mut self, kind: Load) -> Option<Request> {
        let (since, until) = match day_bounds(self.day, self.timezone) {
            Ok(bounds) => bounds,
            Err(error) => {
                self.message = error;
                return None;
            }
        };
        let cursor = match kind {
            Load::Older => self.previous_cursor.clone(),
            Load::Newer => self.next_cursor.clone(),
            _ => None,
        };
        self.generation += 1;
        self.loading = true;
        self.message = "Loading logs...".into();
        self.drag = None;
        if matches!(kind, Load::Latest | Load::Oldest) {
            self.unseen = 0;
        }
        Some(Request {
            started: Instant::now(),
            generation: self.generation,
            kind,
            since,
            until,
            level: LEVELS[self.level],
            limit: self.page_size,
            cursor,
        })
    }

    pub fn accept(&mut self, request: &Request, result: Result<Page, String>) {
        if request.generation != self.generation {
            return;
        }
        self.loading = false;
        let page = match result {
            Ok(page) => page,
            Err(error) => {
                self.message = error;
                return;
            }
        };
        self.initialized = true;
        let count = page.items.len();
        let boundary = CachedPage {
            count,
            before: page.previous_cursor.clone(),
            after: page.next_cursor.clone(),
        };
        self.message = if page.gap {
            format!("History incomplete | source {}", page.source)
        } else if page.skipped_lines > 0 {
            format!(
                "Logs loaded | {} malformed lines skipped",
                page.skipped_lines
            )
        } else {
            "Logs loaded".into()
        };
        match request.kind {
            Load::Latest | Load::Oldest => {
                self.records = page.items.into();
                self.pages.clear();
                if count > 0 {
                    self.pages.push_back(boundary);
                }
                self.previous_cursor = page.previous_cursor;
                self.next_cursor = page.next_cursor;
                self.more_before = page.has_more_before;
                self.more_after = page.has_more_after;
                self.offset = if request.kind == Load::Latest {
                    self.max_offset()
                } else {
                    0
                };
            }
            Load::Older => {
                for record in page.items.into_iter().rev() {
                    self.records.push_front(record);
                }
                self.offset += count;
                if count > 0 {
                    self.pages.push_front(boundary);
                }
                self.previous_cursor = page.previous_cursor;
                self.more_before = page.has_more_before;
                while self.pages.len() > MAX_CACHED_PAGES {
                    let removed = self.pages.pop_back().unwrap();
                    self.records
                        .truncate(self.records.len().saturating_sub(removed.count));
                    self.more_after = true;
                    self.next_cursor = self.pages.back().and_then(|page| page.after.clone());
                }
            }
            Load::Newer => {
                self.records.extend(page.items);
                if count > 0 {
                    self.pages.push_back(boundary);
                }
                self.next_cursor = page.next_cursor;
                self.more_after = page.has_more_after;
                while self.pages.len() > MAX_CACHED_PAGES {
                    let removed = self.pages.pop_front().unwrap();
                    self.records.drain(..removed.count);
                    self.offset = self.offset.saturating_sub(removed.count);
                    self.more_before = true;
                    self.previous_cursor = self.pages.front().and_then(|page| page.before.clone());
                }
            }
        }
        if request.kind == Load::Newer
            && self.day == self.today()
            && self.offset == self.max_offset()
            && !self.more_after
        {
            self.follow = true;
        }
        if self.follow {
            let recent: Vec<_> = self.recent.iter().cloned().collect();
            for record in recent {
                self.append_live(record);
            }
            self.offset = self.max_offset();
            self.unseen = 0;
        }
    }

    fn matches(&self, record: &Value) -> bool {
        LEVELS
            .iter()
            // CRITICAL can be emitted even though it is not a configurable minimum.
            .chain(std::iter::once(&"CRITICAL"))
            .position(|level| record["level"].as_str() == Some(*level))
            .is_some_and(|level| level >= self.level)
            && record["timestamp"]
                .as_str()
                .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
                .is_some_and(|stamp| {
                    local_date(stamp.with_timezone(&Utc), self.timezone) == self.day
                })
    }

    fn append_live(&mut self, record: Value) {
        // Failed HTTP refreshes must not allow the live stream to grow this cache forever.
        // Preserve loaded rows/cursors; maintenance fetches the latest page once HTTP recovers.
        if self.records.len() >= self.page_size * MAX_CACHED_PAGES + RECENT_LIMIT
            || !self.matches(&record)
            || self.records.iter().any(|old| same_record(old, &record))
        {
            return;
        }
        if self.records.back().is_some_and(|last| {
            let old = last["timestamp"]
                .as_str()
                .and_then(|time| DateTime::parse_from_rfc3339(time).ok());
            let new = record["timestamp"]
                .as_str()
                .and_then(|time| DateTime::parse_from_rfc3339(time).ok());
            old > new
                || (old == new
                    && last["server_instance_id"] == record["server_instance_id"]
                    && last["sequence"].as_u64() >= record["sequence"].as_u64())
        }) {
            return;
        }
        self.records.push_back(record);
        if let Some(page) = self.pages.back_mut() {
            page.count += 1;
        }
        self.offset = self.max_offset();
    }

    pub fn record(&mut self, record: Value) -> bool {
        if self.recent.iter().any(|old| same_record(old, &record)) {
            return false;
        }
        self.recent.push_back(record.clone());
        while self.recent.len() > RECENT_LIMIT {
            self.recent.pop_front();
        }
        if self.follow && self.day == self.today() && !self.loading {
            self.append_live(record);
        } else if self.matches(&record) && !self.records.iter().any(|old| same_record(old, &record))
        {
            self.unseen += 1;
        }
        true
    }

    pub fn max_offset(&self) -> usize {
        self.records.len().saturating_sub(self.viewport)
    }

    pub fn layout(&mut self, viewport: usize) {
        self.viewport = viewport;
        if self.follow {
            self.offset = self.max_offset();
        } else {
            self.offset = self.offset.min(self.max_offset());
        }
    }

    fn clear_cache(&mut self) {
        self.records.clear();
        self.pages.clear();
        self.offset = 0;
        self.previous_cursor = None;
        self.next_cursor = None;
        self.more_before = false;
        self.more_after = false;
    }

    pub fn maintenance(&mut self) -> Option<Request> {
        if self.follow
            && !self.loading
            && (self.records.len() > self.page_size * MAX_CACHED_PAGES || self.day != self.today())
        {
            if self.day != self.today() {
                self.clear_cache();
            }
            self.day = self.today();
            self.load(Load::Latest)
        } else {
            None
        }
    }

    pub fn clear_hitboxes(&mut self) {
        self.title = Rect::default();
        self.scrollbar = Rect::default();
        self.thumb = Rect::default();
        self.buttons = [Rect::default(); 2];
    }

    pub fn resize(&mut self) {
        self.title_click = None;
        self.drag = None;
        self.clear_hitboxes();
    }

    pub fn copy_click(&mut self, event: MouseEvent) -> bool {
        if matches!(event.kind, MouseEventKind::Down(_)) {
            let previous = self.title_click.take();
            let position = Position::new(event.column, event.row);
            if event.kind == MouseEventKind::Down(MouseButton::Left)
                && self.title.contains(position)
            {
                if let Some((time, old)) = previous
                    && time.elapsed() <= Duration::from_millis(400)
                    && old.y == position.y
                    && old.x.abs_diff(position.x) <= 1
                {
                    return true;
                }
                self.title_click = Some((Instant::now(), position));
            }
        }
        false
    }

    pub fn clipboard_text(&self) -> String {
        let clean = |text: &str| text.chars().filter(|c| !c.is_control()).collect::<String>();
        let mut text = String::new();
        for record in &self.records {
            let timestamp = record["timestamp"].as_str().unwrap_or("");
            let timestamp = DateTime::parse_from_rfc3339(timestamp)
                .map(|time| match self.timezone {
                    Some(zone) => time.with_timezone(&zone).to_rfc3339(),
                    None => time.with_timezone(&Local).to_rfc3339(),
                })
                .unwrap_or_else(|_| clean(timestamp));
            text.push_str(&timestamp);
            text.push(' ');
            text.push_str(&clean(record["level"].as_str().unwrap_or("INFO")));
            text.push(' ');
            text.push_str(&clean(record["message"].as_str().unwrap_or("")));
            text.push('\n');
        }
        text
    }

    pub fn navigate(&mut self, next: bool) -> Option<Request> {
        let day = if next {
            self.day.checked_add_days(Days::new(1))
        } else {
            self.day.checked_sub_days(Days::new(1))
        }?;
        if day > self.today() {
            return None;
        }
        self.day = day;
        self.follow = false;
        self.clear_cache();
        self.load(if next { Load::Oldest } else { Load::Latest })
    }

    fn start_archive(&mut self) -> Option<Request> {
        self.follow = false;
        self.load(Load::Oldest)
    }

    pub fn scroll(&mut self, down: bool, amount: usize) -> Option<Request> {
        if self.loading {
            return None;
        }
        let maximum = self.max_offset();
        if !down {
            self.follow = false;
            if self.offset > 0 {
                self.offset = self.offset.saturating_sub(amount);
            } else if self.more_before {
                return self.load(Load::Older);
            } else {
                return self.navigate(false);
            }
        } else if self.offset < maximum {
            self.offset = (self.offset + amount).min(maximum);
        } else if self.more_after {
            return self.load(Load::Newer);
        } else if self.day != self.today() {
            return self.navigate(true);
        }
        if down && self.offset == self.max_offset() && !self.more_after && self.day == self.today()
        {
            self.follow = true;
            if self.unseen > 0 {
                return self.load(Load::Latest);
            }
        }
        None
    }

    pub fn key(&mut self, key: KeyCode) -> Option<Request> {
        match key {
            KeyCode::Char('[') | KeyCode::Left => self.navigate(false),
            KeyCode::Char(']') | KeyCode::Right => self.navigate(true),
            KeyCode::End => {
                if self.day != self.today() {
                    self.clear_cache();
                }
                self.day = self.today();
                self.follow = true;
                self.load(Load::Latest)
            }
            KeyCode::Home => self.start_archive(),
            KeyCode::Up | KeyCode::PageUp => self.scroll(
                false,
                if key == KeyCode::Up {
                    1
                } else {
                    self.viewport.max(1)
                },
            ),
            KeyCode::Down | KeyCode::PageDown => self.scroll(
                true,
                if key == KeyCode::Down {
                    1
                } else {
                    self.viewport.max(1)
                },
            ),
            KeyCode::Char('f') => {
                self.level = (self.level + 1) % LEVELS.len();
                self.clear_cache();
                self.start_archive()
            }
            KeyCode::Char('+' | '=') | KeyCode::Char('-') => {
                let index = PAGE_SIZES
                    .iter()
                    .position(|&size| size == self.page_size)
                    .unwrap();
                let index = if key == KeyCode::Char('-') {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(PAGE_SIZES.len() - 1)
                };
                self.page_size = PAGE_SIZES[index];
                self.start_archive()
            }
            _ => None,
        }
    }

    pub fn mouse(&mut self, event: MouseEvent) -> Option<Request> {
        let position = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::ScrollUp => self.scroll(false, 3),
            MouseEventKind::ScrollDown => self.scroll(true, 3),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = self.buttons.iter().position(|rect| rect.contains(position)) {
                    return self.navigate(index == 1);
                }
                if self.scrollbar.contains(position) {
                    self.follow = false;
                    self.drag = Some(if self.thumb.contains(position) {
                        event.row - self.thumb.y
                    } else {
                        self.thumb.height / 2
                    });
                    self.drag_to(event.row);
                }
                None
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.drag_to(event.row);
                None
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.drag.take().is_some() {
                    if self.offset == 0 && self.more_before {
                        return self.load(Load::Older);
                    }
                    if self.offset == self.max_offset() {
                        if self.more_after {
                            return self.load(Load::Newer);
                        }
                        if self.day == self.today() {
                            self.follow = true;
                            if self.unseen > 0 {
                                return self.load(Load::Latest);
                            }
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn drag_to(&mut self, row: u16) {
        if let Some(grab) = self.drag {
            let travel = self.scrollbar.height.saturating_sub(self.thumb.height);
            let position = row.saturating_sub(self.scrollbar.y + grab).min(travel);
            if travel > 0 {
                self.offset = (usize::from(position) * self.max_offset() + usize::from(travel) / 2)
                    / usize::from(travel);
            }
        }
    }
}

fn same_record(left: &Value, right: &Value) -> bool {
    left["server_instance_id"] == right["server_instance_id"]
        && left["sequence"] == right["sequence"]
}

fn local_date(time: DateTime<Utc>, timezone: Option<chrono_tz::Tz>) -> NaiveDate {
    timezone
        .map(|zone| time.with_timezone(&zone).date_naive())
        .unwrap_or_else(|| time.with_timezone(&Local).date_naive())
}

pub fn day_bounds(
    day: NaiveDate,
    timezone: Option<chrono_tz::Tz>,
) -> Result<(String, String), String> {
    fn boundary(day: NaiveDate, timezone: Option<chrono_tz::Tz>) -> Option<DateTime<Utc>> {
        let midnight = day.and_hms_opt(0, 0, 0)?;
        // Some IANA zones skip or repeat midnight. Use the first valid instant in that date.
        for minute in 0..1440 {
            let local = midnight + chrono::Duration::minutes(minute);
            let instant = match timezone {
                Some(zone) => zone
                    .from_local_datetime(&local)
                    .earliest()
                    .map(|time| time.with_timezone(&Utc)),
                None => Local
                    .from_local_datetime(&local)
                    .earliest()
                    .map(|time| time.with_timezone(&Utc)),
            };
            if instant.is_some() {
                return instant;
            }
        }
        None
    }
    let start = boundary(day, timezone).ok_or("Selected day has no valid local midnight.")?;
    let end = day
        .succ_opt()
        .and_then(|next| boundary(next, timezone))
        .ok_or("Cannot determine the next day boundary.")?;
    Ok((start.to_rfc3339(), end.to_rfc3339()))
}
