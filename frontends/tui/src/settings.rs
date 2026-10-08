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
#[serde(default)]
pub struct BatteryAlert {
    pub enabled: bool,
    pub threshold_percent: u8,
    pub hysteresis_percent: u8,
    pub min_notification_interval_minutes: u16,
}

impl Default for BatteryAlert {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_percent: 20,
            hysteresis_percent: 5,
            min_notification_interval_minutes: 10,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ConnectionAlert {
    pub enabled: bool,
    pub outage_seconds: u32,
    pub recovery_seconds: u32,
    pub min_notification_interval_minutes: u16,
}

impl Default for ConnectionAlert {
    fn default() -> Self {
        Self {
            enabled: true,
            outage_seconds: 60,
            recovery_seconds: 15,
            min_notification_interval_minutes: 10,
        }
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
    #[serde(default)]
    pub battery_alert: BatteryAlert,
    #[serde(default)]
    pub connection_alert: ConnectionAlert,
    #[serde(default, skip_serializing)]
    pub telegram_configured: bool,
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
            battery_alert: BatteryAlert::default(),
            connection_alert: ConnectionAlert::default(),
            telegram_configured: false,
        }
    }
}

impl Settings {
    pub fn valid(&self) -> bool {
        self.resolution().is_some()
            && [100, 300].contains(&self.graph_base_scale_w)
            && [50, 100, 250, 500, 1000].contains(&self.logs_page_size)
            && (self.timezone == "system" || self.timezone.parse::<chrono_tz::Tz>().is_ok())
            && (1..=86400).contains(&self.connection_alert.outage_seconds)
            && (1..=3600).contains(&self.connection_alert.recovery_seconds)
            && self.connection_alert.min_notification_interval_minutes <= 1440
            && self.battery_alert.threshold_percent < 100
            && self.battery_alert.hysteresis_percent > 0
            && u16::from(self.battery_alert.threshold_percent)
                + u16::from(self.battery_alert.hysteresis_percent)
                <= 100
            && self.battery_alert.min_notification_interval_minutes <= 1440
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
            Field::Theme => "MyPowers".into(),
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
            Field::AlertEnabled => if self.battery_alert.enabled {
                "Enabled"
            } else {
                "Disabled"
            }
            .into(),
            Field::ConnectionEnabled => if self.connection_alert.enabled {
                "En"
            } else {
                "Dis"
            }
            .into(),
            Field::ConnectionDelay => format!("{} s", self.connection_alert.outage_seconds),
            Field::ConnectionRecovery => format!("{} s", self.connection_alert.recovery_seconds),
            Field::ConnectionCooldown => format!(
                "{} min",
                self.connection_alert.min_notification_interval_minutes
            ),
            Field::AlertThreshold => format!("{}%", self.battery_alert.threshold_percent),
            Field::AlertHysteresis => format!("{} pp", self.battery_alert.hysteresis_percent),
            Field::AlertCooldown => format!(
                "{} min",
                self.battery_alert.min_notification_interval_minutes
            ),
        }
    }

    pub fn choose(&mut self, field: Field, index: usize) {
        match field {
            Field::Theme => {}
            Field::Visualization => {
                self.graph_visualization = [Visualization::Sparkline, Visualization::Chart][index]
            }
            Field::Interval => self.graph_interval_seconds = [10, 30, 60, 3600][index],
            Field::Scale => self.graph_base_scale_w = [100, 300][index],
            Field::PageSize => self.logs_page_size = [50, 100, 250, 500, 1000][index],
            Field::AlertEnabled => self.battery_alert.enabled = index == 0,
            Field::ConnectionEnabled => self.connection_alert.enabled = index == 0,
            Field::ConnectionDelay => self.connection_alert.outage_seconds = (index + 1) as u32,
            Field::ConnectionRecovery => {
                self.connection_alert.recovery_seconds = (index + 1) as u32
            }
            Field::ConnectionCooldown => {
                self.connection_alert.min_notification_interval_minutes = index as u16
            }
            Field::AlertThreshold => self.battery_alert.threshold_percent = index as u8,
            Field::AlertHysteresis => self.battery_alert.hysteresis_percent = (index + 1) as u8,
            Field::AlertCooldown => {
                self.battery_alert.min_notification_interval_minutes = index as u16
            }
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
    Theme,
    Visualization,
    Interval,
    Scale,
    Timezone,
    PageSize,
    AlertEnabled,
    AlertThreshold,
    AlertHysteresis,
    AlertCooldown,
    ConnectionEnabled,
    ConnectionDelay,
    ConnectionRecovery,
    ConnectionCooldown,
}

impl Field {
    pub const CHARTS: [Self; 3] = [Self::Visualization, Self::Interval, Self::Scale];
    pub const PREFERENCES: [Self; 3] = [Self::Timezone, Self::PageSize, Self::Theme];
    pub const ALERTS: [Self; 8] = [
        Self::AlertEnabled,
        Self::AlertThreshold,
        Self::AlertHysteresis,
        Self::AlertCooldown,
        Self::ConnectionEnabled,
        Self::ConnectionDelay,
        Self::ConnectionRecovery,
        Self::ConnectionCooldown,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "Theme",
            Self::Visualization => "Visualization",
            Self::Interval => "Interval per bar",
            Self::Scale => "Base scale (auto)",
            Self::Timezone => "Timezone",
            Self::PageSize => "Logs page size",
            Self::AlertEnabled => "Battery alert",
            Self::AlertThreshold => "Low threshold",
            Self::AlertHysteresis => "Hysteresis",
            Self::AlertCooldown => "Cooldown",
            Self::ConnectionEnabled => "Connection alert",
            Self::ConnectionDelay => "Alert after",
            Self::ConnectionRecovery => "Stable recovery",
            Self::ConnectionCooldown => "Cooldown",
        }
    }
    pub fn is_segmented(self) -> bool {
        matches!(
            self,
            Self::Visualization | Self::Scale | Self::AlertEnabled | Self::ConnectionEnabled
        )
    }

    pub fn segment_label(self, index: usize) -> String {
        if self == Self::Scale {
            format!("{} W", [100, 300][index])
        } else {
            self.choices()[index].clone()
        }
    }

    pub fn choices(self) -> Vec<String> {
        match self {
            Self::Theme => crate::client_ui::THEMES.map(String::from).to_vec(),
            Self::Visualization => vec!["Sparkline".into(), "Chart".into()],
            Self::Interval => ["10s", "30s", "60s", "1h"].map(String::from).to_vec(),
            Self::Scale => ["0–100 W (auto)", "0–300 W (auto)"]
                .map(String::from)
                .to_vec(),
            Self::PageSize => [50, 100, 250, 500, 1000].map(|v| v.to_string()).to_vec(),
            Self::ConnectionEnabled => vec!["En".into(), "Dis".into()],
            Self::ConnectionDelay => (1..=86400).map(|v| format!("{v} s")).collect(),
            Self::ConnectionRecovery => (1..=3600).map(|v| format!("{v} s")).collect(),
            Self::ConnectionCooldown => (0..=1440).map(|v| format!("{v} min")).collect(),
            Self::AlertEnabled => vec!["Enabled".into(), "Disabled".into()],
            Self::AlertThreshold => (0..100).map(|v| format!("{v}%")).collect(),
            Self::AlertHysteresis => (1..=100).map(|v| format!("{v} pp")).collect(),
            Self::AlertCooldown => (0..=1440).map(|v| format!("{v} min")).collect(),
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
