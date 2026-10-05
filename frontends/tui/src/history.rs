//! Read-only graph backfill from the daemon's persisted telemetry API.
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use std::time::Instant;

pub const WINDOW_SECONDS: i64 = 120;
pub const MAX_POINTS: usize = 10_000;
pub const PAGE_SIZE: usize = 1000;

#[derive(Clone, Debug)]
pub struct Request {
    pub generation: u64,
    pub server_instance_id: String,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub started: Instant,
}

impl Request {
    pub fn new(status: &crate::model::Status, generation: u64) -> Option<Self> {
        let until = DateTime::parse_from_rfc3339(&status.server_time)
            .ok()?
            .with_timezone(&Utc);
        Some(Self {
            generation,
            server_instance_id: status.server_instance_id.clone(),
            since: until.checked_sub_signed(TimeDelta::seconds(WINDOW_SECONDS))?,
            until,
            started: Instant::now(),
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct Point {
    pub id: u64,
    pub received_at_ms: i64,
    pub segment_id: String,
    pub input_power_w: u64,
    pub output_power_w: u64,
}

impl Point {
    pub fn valid(&self, request: &Request) -> bool {
        self.id > 0
            && !self.segment_id.is_empty()
            && self.input_power_w <= 65535
            && self.output_power_w <= 65535
            && (request.since.timestamp_millis()..request.until.timestamp_millis())
                .contains(&self.received_at_ms)
    }
}

#[derive(Deserialize)]
pub struct Page {
    pub schema_version: u8,
    pub items: Vec<Point>,
    pub next_cursor: Option<String>,
    pub source: String,
    pub gap: bool,
}
