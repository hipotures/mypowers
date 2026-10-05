use crate::model::{Command, safe};
use serde_json::Value;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

pub struct Feedback {
    pub message: String,
    pub severity: Severity,
    pub started: Instant,
}

impl Feedback {
    pub fn new(message: impl Into<String>, severity: Severity) -> Self {
        Self {
            message: safe(&message.into()),
            severity,
            started: Instant::now(),
        }
    }

    pub fn stage(&self) -> Option<u8> {
        let seconds = self.started.elapsed().as_secs();
        (seconds < 8).then_some((seconds / 2) as u8)
    }

    pub fn command(command: &Command) -> Self {
        let label = output_name(&command.output);
        let state = if command.requested_enabled {
            "ON"
        } else {
            "OFF"
        };
        let (message, severity) = match command.status.as_str() {
            "confirmed" | "no_change" => (format!("{label} {state} confirmed"), Severity::Success),
            "unconfirmed" => (
                format!("{label} {state} not confirmed; check station"),
                Severity::Warning,
            ),
            "failed" | "rejected" => (
                format!(
                    "{label} {state} failed: {}",
                    reason(command.reason_code.as_deref().unwrap_or(""))
                ),
                Severity::Error,
            ),
            _ => (
                format!("Waiting for {label} {state} confirmation..."),
                Severity::Info,
            ),
        };
        Self::new(message, severity)
    }

    pub fn request_error(error: &str) -> Self {
        let message = if let Some((_, code)) = error.split_once(": ") {
            format!("Command failed: {}", reason(code))
        } else {
            "Request failed; check daemon connection".into()
        };
        Self::new(message, Severity::Error)
    }

    pub fn log(record: &Value) -> Option<Self> {
        let severity = match record["level"].as_str()? {
            "INFO" => Severity::Info,
            "WARNING" => Severity::Warning,
            "ERROR" => Severity::Error,
            _ => return None,
        };
        // Connection and output events have more precise feedback in the state/command stream.
        let message = match record["event"].as_str().unwrap_or("") {
            "connection_transition" | "command_outcome" => return None,
            "log_level_changed" => {
                let level = record["context"]["effective"].as_str()?;
                if !["DEBUG", "INFO", "WARNING", "ERROR"].contains(&level) {
                    return None;
                }
                format!("Log level changed to {level}")
            }
            "storage_failure" => "History storage unavailable; see Logs".into(),
            "essential_task_failed" => "Daemon task stopped; see Logs".into(),
            _ => record["message"].as_str()?.to_owned(),
        };
        (!message.is_empty()).then(|| Self::new(message, severity))
    }
}

pub fn output_name(output: &str) -> &'static str {
    match output {
        "ac" => "AC",
        "dc" => "DC",
        "light" => "Lamps",
        _ => "Output",
    }
}

fn reason(code: &str) -> &'static str {
    match code {
        "telemetry_unavailable" | "stale_telemetry" => "telemetry unavailable",
        "connection_lost" => "connection lost",
        "command_busy" => "another command is pending",
        "resynchronizing" => "waiting for fresh station data",
        "shutdown" => "daemon stopped",
        "transport_or_status_timeout" => "station did not respond",
        "confirmation_contradiction" => "station reported a different state",
        "revision_conflict" | "stale_revision" | "instance_mismatch" => {
            "station state changed; try again"
        }
        "unauthorized" | "forbidden" => "access denied",
        _ => "request rejected; see Logs",
    }
}
