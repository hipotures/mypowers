//! Bounded server-computed power averages; the frontend never reads raw history.
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::time::Instant;

pub const MAX_BUCKETS: u16 = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Visualization {
    #[default]
    Sparkline,
    Chart,
}

impl Visualization {
    pub fn next(self) -> Self {
        match self {
            Self::Sparkline => Self::Chart,
            Self::Chart => Self::Sparkline,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sparkline => "Sparkline",
            Self::Chart => "Chart",
        }
    }

    pub fn height(self) -> u16 {
        match self {
            Self::Sparkline => 2,
            // Six plot rows plus the horizontal axis and its labels.
            Self::Chart => 8,
        }
    }
}

/// Start at 100 W and double until every visible value fits.
pub fn power_scale(values: impl IntoIterator<Item = f64>) -> u64 {
    let peak = values.into_iter().fold(0.0_f64, f64::max);
    let mut maximum = 100;
    while peak > maximum as f64 {
        maximum *= 2;
    }
    maximum
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resolution {
    #[default]
    TenSeconds,
    Minute,
    Hour,
}

impl Resolution {
    pub fn seconds(self) -> i64 {
        match self {
            Self::TenSeconds => 10,
            Self::Minute => 60,
            Self::Hour => 3600,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::TenSeconds => "10s",
            Self::Minute => "60s",
            Self::Hour => "1h",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::TenSeconds => Self::Minute,
            Self::Minute => Self::Hour,
            Self::Hour => Self::TenSeconds,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub generation: u64,
    pub server_instance_id: String,
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub resolution: Resolution,
    pub width: u16,
    pub started: Instant,
}

impl Request {
    pub fn new(
        status: &crate::model::Status,
        generation: u64,
        resolution: Resolution,
        width: u16,
        now_ms: i64,
    ) -> Option<Self> {
        if !(1..=MAX_BUCKETS).contains(&width) {
            return None;
        }
        let span = resolution.seconds() * 1000;
        let start = now_ms.div_euclid(span) * span - i64::from(width - 1) * span;
        Some(Self {
            generation,
            server_instance_id: status.server_instance_id.clone(),
            since: DateTime::from_timestamp_millis(start)?,
            // Include a persisted observation exactly at the current server timestamp.
            until: DateTime::from_timestamp_millis(now_ms.checked_add(1)?)?,
            resolution,
            width,
            started: Instant::now(),
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Point {
    pub bucket_start_ms: i64,
    pub input_power_w: f64,
    pub output_power_w: f64,
    pub sample_count: u64,
}

impl Point {
    pub fn valid(&self, request: &Request) -> bool {
        self.sample_count > 0
            && self.input_power_w.is_finite()
            && self.output_power_w.is_finite()
            && (0.0..=65535.0).contains(&self.input_power_w)
            && (0.0..=65535.0).contains(&self.output_power_w)
            && self
                .bucket_start_ms
                .rem_euclid(request.resolution.seconds() * 1000)
                == 0
            && (request.since.timestamp_millis()..request.until.timestamp_millis())
                .contains(&self.bucket_start_ms)
    }
}

#[derive(Deserialize)]
pub struct Response {
    pub schema_version: u8,
    pub bucket_seconds: i64,
    pub since_ms: i64,
    pub until_ms: i64,
    pub items: Vec<Point>,
    pub source: String,
}

pub struct Graph {
    pub visualization: Visualization,
    pub resolution: Resolution,
    pub width: u16,
    pub points: Vec<Point>,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            visualization: Visualization::default(),
            resolution: Resolution::default(),
            width: 43,
            points: Vec::new(),
        }
    }
}

impl Graph {
    pub fn data(&self, now_ms: i64, width: u16, output: bool) -> Vec<f64> {
        self.columns(now_ms, width, output)
            .into_iter()
            .map(|value| value.unwrap_or(0.0))
            .collect()
    }

    /// Keep missing buckets distinct from measured zeros for line-chart breaks.
    pub fn columns(&self, now_ms: i64, width: u16, output: bool) -> Vec<Option<f64>> {
        let mut values = vec![None; usize::from(width)];
        let first = now_ms.div_euclid(self.resolution.seconds() * 1000) - i64::from(width) + 1;
        for point in &self.points {
            let column = point
                .bucket_start_ms
                .div_euclid(self.resolution.seconds() * 1000)
                - first;
            if (0..i64::from(width)).contains(&column) {
                values[column as usize] = Some(if output {
                    point.output_power_w
                } else {
                    point.input_power_w
                });
            }
        }
        values
    }

    pub fn prune(&mut self, now_ms: i64) {
        let first = (now_ms.div_euclid(self.resolution.seconds() * 1000) - i64::from(self.width)
            + 1)
            * self.resolution.seconds()
            * 1000;
        self.points.retain(|point| point.bucket_start_ms >= first);
    }
}
