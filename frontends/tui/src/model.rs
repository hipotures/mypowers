use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Sample {
    pub sequence: u64,
    pub received_at: String,
    pub segment_id: String,
    pub battery_percent: u16,
    pub input_power_w: u64,
    pub output_power_w: u64,
    pub remaining_minutes: u64,
    pub ac_enabled: bool,
    pub dc_enabled: bool,
    pub light_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Connection {
    pub phase: String,
    pub desired: String,
    pub link_connected: bool,
    pub session_id: Option<String>,
    pub message: String,
    pub adapter_id: Option<String>,
    pub adapter_address: Option<String>,
}

impl Connection {
    pub fn valid(&self) -> bool {
        matches!(self.desired.as_str(), "running" | "paused")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Telemetry {
    pub state: String,
    pub age_seconds: Option<f64>,
    pub sample: Option<Sample>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Controls {
    pub allowed: bool,
    pub outputs_revision: u64,
    pub pending_command_id: Option<String>,
    pub reason_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Status {
    pub schema_version: u8,
    pub server_time: String,
    pub server_instance_id: String,
    pub state_version: u64,
    pub device: Value,
    pub connection: Connection,
    pub telemetry: Telemetry,
    pub controls: Controls,
    pub history: Value,
    pub logging: Value,
}

impl Status {
    pub fn valid(&self) -> bool {
        self.schema_version == 1
            && self.connection.valid()
            && chrono::DateTime::parse_from_rfc3339(&self.server_time).is_ok()
            && uuid::Uuid::parse_str(&self.server_instance_id).is_ok()
            && matches!(
                self.telemetry.state.as_str(),
                "unknown" | "waiting" | "live" | "stale" | "invalid"
            )
            && self
                .telemetry
                .age_seconds
                .is_none_or(|age| age.is_finite() && age >= 0.0)
            && (self.telemetry.state != "live" || self.telemetry.sample.is_some())
            && self.telemetry.sample.as_ref().is_none_or(|s| {
                s.battery_percent <= 100
                    && s.input_power_w <= 65535
                    && s.output_power_w <= 65535
                    && s.remaining_minutes <= 65535
                    && chrono::DateTime::parse_from_rfc3339(&s.received_at).is_ok()
            })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Command {
    pub schema_version: u8,
    pub command_id: String,
    pub status: String,
    pub output: String,
    pub requested_enabled: bool,
    pub reason_code: Option<String>,
}

impl Command {
    pub fn valid(&self) -> bool {
        self.schema_version == 1
            && uuid::Uuid::parse_str(&self.command_id).is_ok()
            && matches!(self.output.as_str(), "ac" | "dc" | "light")
            && matches!(
                self.status.as_str(),
                "accepted"
                    | "waiting_for_status"
                    | "sent"
                    | "confirmed"
                    | "no_change"
                    | "rejected"
                    | "failed"
                    | "unconfirmed"
            )
    }
    pub fn terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "confirmed" | "no_change" | "rejected" | "failed" | "unconfirmed"
        )
    }
}

#[derive(Deserialize)]
pub struct StreamMessage {
    pub schema_version: u8,
    #[serde(rename = "type")]
    pub kind: String,
    pub server_instance_id: String,
    pub stream_sequence: u64,
    pub server_time: String,
    pub data: Option<Value>,
}

pub fn safe(value: &str) -> Cow<'_, str> {
    for (index, character) in value.chars().enumerate() {
        if index == 2000 || character.is_control() {
            return Cow::Owned(
                value
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(2000)
                    .collect(),
            );
        }
    }
    Cow::Borrowed(value)
}
