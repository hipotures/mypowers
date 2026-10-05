use crate::history::Resolution;
use serde::Deserialize;

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

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    pub schema_version: u8,
    pub graph_interval_seconds: i64,
}

impl Settings {
    pub fn resolution(&self) -> Option<Resolution> {
        if self.schema_version != 1 {
            return None;
        }
        match self.graph_interval_seconds {
            10 => Some(Resolution::TenSeconds),
            60 => Some(Resolution::Minute),
            3600 => Some(Resolution::Hour),
            _ => None,
        }
    }
}
