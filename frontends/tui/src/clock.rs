//! Rendering time is explicit so offline fixtures never depend on the system clock.
use chrono::{DateTime, Utc};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Default)]
pub enum Clock {
    #[default]
    Live,
    Fixed {
        now: DateTime<Utc>,
        telemetry_elapsed: Duration,
        animation_elapsed: Duration,
        feedback_elapsed: Duration,
    },
}

impl Clock {
    pub fn now(self) -> DateTime<Utc> {
        match self {
            Self::Live => Utc::now(),
            Self::Fixed { now, .. } => now,
        }
    }

    pub fn telemetry_elapsed(self, received: Instant) -> Duration {
        match self {
            Self::Live => received.elapsed(),
            Self::Fixed {
                telemetry_elapsed, ..
            } => telemetry_elapsed,
        }
    }

    pub fn animation_elapsed(self, started: Instant) -> Duration {
        match self {
            Self::Live => started.elapsed(),
            Self::Fixed {
                animation_elapsed, ..
            } => animation_elapsed,
        }
    }

    pub fn feedback_elapsed(self, started: Instant) -> Duration {
        match self {
            Self::Live => started.elapsed(),
            Self::Fixed {
                feedback_elapsed, ..
            } => feedback_elapsed,
        }
    }
}
