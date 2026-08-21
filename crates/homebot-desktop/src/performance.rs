//! Local-only performance measurements and release budgets.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub const COLD_START_BUDGET: Duration = Duration::from_secs(5);
pub const RECONNECT_BUDGET: Duration = Duration::from_secs(2);
pub const CHAT_OPEN_BUDGET: Duration = Duration::from_millis(250);
pub const STREAM_FRAME_BUDGET: Duration = Duration::from_millis(50);
pub const IDLE_CPU_PERCENT_BUDGET: f32 = 2.0;
pub const IDLE_RSS_MIB_BUDGET: u64 = 250;
pub const LARGE_TRANSCRIPT_MESSAGES: usize = 10_000;
pub const CONCURRENT_BOT_BUDGET: usize = 8;
const SAMPLE_CAPACITY: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PerformanceSample {
    pub operation: &'static str,
    pub elapsed: Duration,
    pub observed_at: Instant,
}

/// In-memory telemetry. Samples are bounded, never transmitted, and disappear
/// when the desktop process exits.
#[derive(Debug, Default)]
pub struct LocalPerformanceTelemetry {
    samples: VecDeque<PerformanceSample>,
}

impl LocalPerformanceTelemetry {
    pub fn record(&mut self, operation: &'static str, elapsed: Duration) {
        if self.samples.len() == SAMPLE_CAPACITY {
            self.samples.pop_front();
        }
        tracing::debug!(
            target: "homebot.performance",
            operation,
            elapsed_ms = elapsed.as_secs_f64() * 1_000.0,
            "local performance sample"
        );
        self.samples.push_back(PerformanceSample {
            operation,
            elapsed,
            observed_at: Instant::now(),
        });
    }

    #[must_use]
    pub fn samples(&self) -> &VecDeque<PerformanceSample> {
        &self.samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_is_bounded_and_local_only() {
        let mut telemetry = LocalPerformanceTelemetry::default();
        for index in 0..(SAMPLE_CAPACITY + 20) {
            telemetry.record("desktop_frame", Duration::from_micros(index as u64));
        }
        assert_eq!(telemetry.samples().len(), SAMPLE_CAPACITY);
        assert_eq!(
            telemetry.samples().front().map(|sample| sample.elapsed),
            Some(Duration::from_micros(20))
        );
    }
}
