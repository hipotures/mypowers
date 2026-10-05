use crate::history::{Resolution, Visualization};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    Preferences,
    Charts,
    Alerts,
    Notify,
    Debug,
}

impl SettingsTab {
    pub const ALL: [Self; 5] = [
        Self::Preferences,
        Self::Charts,
        Self::Alerts,
        Self::Notify,
        Self::Debug,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Preferences => "Preferences",
            Self::Charts => "Charts",
            Self::Alerts => "Alerts",
            Self::Notify => "Notify",
            Self::Debug => "Debug",
        }
    }

    pub fn next(self, backwards: bool) -> Self {
        Self::ALL
            [(self as usize + if backwards { Self::ALL.len() - 1 } else { 1 }) % Self::ALL.len()]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Settings {
    pub schema_version: u8,
    pub graph_interval_seconds: i64,
    pub graph_visualization: Visualization,
    pub graph_base_scale_w: u64,
    pub timezone: String,
    pub logs_page_size: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            graph_interval_seconds: 10,
            graph_visualization: Visualization::Sparkline,
            graph_base_scale_w: 100,
            timezone: "system".into(),
            logs_page_size: 100,
        }
    }
}

impl Settings {
    pub fn valid(&self) -> bool {
        self.resolution().is_some()
            && [100, 300].contains(&self.graph_base_scale_w)
            && [50, 100, 250, 500, 1000].contains(&self.logs_page_size)
            && (self.timezone == "system" || self.timezone.parse::<chrono_tz::Tz>().is_ok())
    }

    pub fn resolution(&self) -> Option<Resolution> {
        if self.schema_version != 1 {
            return None;
        }
        match self.graph_interval_seconds {
            10 => Some(Resolution::TenSeconds),
            30 => Some(Resolution::ThirtySeconds),
            60 => Some(Resolution::Minute),
            3600 => Some(Resolution::Hour),
            _ => None,
        }
    }

    pub fn value(&self, field: Field) -> String {
        match field {
            Field::Visualization => self.graph_visualization.label().into(),
            Field::Interval => self.resolution().unwrap_or_default().label().into(),
            Field::Scale => format!("0–{} W (auto)", self.graph_base_scale_w),
            Field::Timezone => {
                if self.timezone == "system" {
                    "System local".into()
                } else {
                    self.timezone.clone()
                }
            }
            Field::PageSize => self.logs_page_size.to_string(),
        }
    }

    pub fn choose(&mut self, field: Field, index: usize) {
        match field {
            Field::Visualization => {
                self.graph_visualization = [Visualization::Sparkline, Visualization::Chart][index]
            }
            Field::Interval => self.graph_interval_seconds = [10, 30, 60, 3600][index],
            Field::Scale => self.graph_base_scale_w = [100, 300][index],
            Field::PageSize => self.logs_page_size = [50, 100, 250, 500, 1000][index],
            Field::Timezone => {
                self.timezone = if index == 0 {
                    "system".into()
                } else {
                    chrono_tz::TZ_VARIANTS[index - 1].name().into()
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Visualization,
    Interval,
    Scale,
    Timezone,
    PageSize,
}

impl Field {
    pub const CHARTS: [Self; 3] = [Self::Visualization, Self::Interval, Self::Scale];
    pub const PREFERENCES: [Self; 2] = [Self::Timezone, Self::PageSize];
    pub fn label(self) -> &'static str {
        match self {
            Self::Visualization => "Visualization",
            Self::Interval => "Interval per bar",
            Self::Scale => "Base scale",
            Self::Timezone => "Timezone",
            Self::PageSize => "Logs page size",
        }
    }
    pub fn choices(self) -> Vec<String> {
        match self {
            Self::Visualization => vec!["Sparkline".into(), "Chart".into()],
            Self::Interval => ["10s", "30s", "60s", "1h"].map(String::from).to_vec(),
            Self::Scale => ["0–100 W (auto)", "0–300 W (auto)"]
                .map(String::from)
                .to_vec(),
            Self::PageSize => [50, 100, 250, 500, 1000].map(|v| v.to_string()).to_vec(),
            Self::Timezone => std::iter::once("System local".into())
                .chain(
                    chrono_tz::TZ_VARIANTS
                        .iter()
                        .map(|zone| zone.name().to_owned()),
                )
                .collect(),
        }
    }
}

pub struct Picker {
    pub field: Field,
    pub selected: usize,
    pub query: String,
}
impl Picker {
    pub fn options(&self) -> Vec<(usize, String)> {
        let query = self.query.to_lowercase();
        let mut options: Vec<_> = self
            .field
            .choices()
            .into_iter()
            .enumerate()
            .filter(|(_, value)| value.to_lowercase().contains(&query))
            .collect();
        if !query.is_empty() {
            options.sort_by_key(|(_, value)| !value.eq_ignore_ascii_case(&query));
        }
        options
    }
}
