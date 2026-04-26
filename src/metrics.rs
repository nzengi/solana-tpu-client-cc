//! Metrics collection and reporting for the TPU congestion controller.

use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::types::{CongestionPhase, RttEstimate, SendStats, WindowState};

// ─── Snapshot ───────────────────────────────────────────────────────────────

/// A point-in-time snapshot of all observable controller metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub window:        WindowState,
    pub rtt:           RttEstimate,
    pub stats:         SendStats,
    pub elapsed_secs:  f64,
}

impl MetricsSnapshot {
    /// Transactions per second (acknowledged / elapsed).
    pub fn tps(&self) -> f64 {
        if self.elapsed_secs <= 0.0 {
            return 0.0;
        }
        self.stats.acknowledged as f64 / self.elapsed_secs
    }
}

// ─── Recorder ───────────────────────────────────────────────────────────────

/// Accumulates per-operation events into `SendStats`.
#[derive(Debug)]
pub struct MetricsRecorder {
    stats:   SendStats,
    started: Instant,
}

impl MetricsRecorder {
    pub fn new() -> Self {
        Self {
            stats:   SendStats::default(),
            started: Instant::now(),
        }
    }

    pub fn record_submit(&mut self) {
        self.stats.submitted += 1;
    }

    pub fn record_ack(&mut self) {
        self.stats.acknowledged += 1;
    }

    pub fn record_backpressure(&mut self) {
        self.stats.backpressured += 1;
    }

    pub fn record_window_increase(&mut self, new_size: usize) {
        self.stats.increases += 1;
        if new_size > self.stats.peak_window {
            self.stats.peak_window = new_size;
        }
    }

    pub fn record_window_decrease(&mut self) {
        self.stats.reductions += 1;
    }

    pub fn set_window_state(&mut self, w: WindowState) {
        self.stats.window = Some(w);
    }

    pub fn set_rtt(&mut self, rtt: RttEstimate) {
        self.stats.rtt = rtt;
    }

    pub fn snapshot(&self, window: WindowState, rtt: RttEstimate) -> MetricsSnapshot {
        MetricsSnapshot {
            window,
            rtt,
            stats:        self.stats.clone(),
            elapsed_secs: self.started.elapsed().as_secs_f64(),
        }
    }
}

impl Default for MetricsRecorder {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Display ────────────────────────────────────────────────────────────────

/// Render a human-readable metrics report to a `String`.
pub fn format_metrics(snap: &MetricsSnapshot) -> String {
    let phase_str = match snap.window.phase {
        CongestionPhase::SlowStart          => "slow-start",
        CongestionPhase::CongestionAvoidance => "congestion-avoidance",
        CongestionPhase::Recovery           => "recovery",
    };

    let bar_width = 40usize;
    let fill = ((snap.window.current_window as f64 / crate::types::WINDOW_MAX as f64)
        * bar_width as f64) as usize;
    let bar: String = "█".repeat(fill) + &"░".repeat(bar_width - fill);

    format!(
        "\n  TPU congestion-control metrics\n\
         ─────────────────────────────────────────────\n\
           window      {:>6}  tx   (in-flight: {})\n\
           ssthresh    {:>6}  tx\n\
           phase       {}\n\
           RTT         {:>7.1} ms  (samples: {})\n\
         ─────────────────────────────────────────────\n\
           submitted   {:>8}\n\
           acked       {:>8}\n\
           backpressed {:>8}  ({:.1}%)\n\
           increases   {:>8}\n\
           reductions  {:>8}\n\
           peak window {:>8}\n\
           elapsed     {:>7.1} s\n\
           TPS         {:>7.1}\n\
         ─────────────────────────────────────────────\n\
           utilization  [{bar}] {:.0}%\n",
        snap.window.current_window,
        snap.window.in_flight,
        snap.window.ssthresh,
        phase_str,
        snap.rtt.smoothed_ms,
        snap.rtt.samples,
        snap.stats.submitted,
        snap.stats.acknowledged,
        snap.stats.backpressured,
        snap.stats.drop_rate() * 100.0,
        snap.stats.increases,
        snap.stats.reductions,
        snap.stats.peak_window,
        snap.elapsed_secs,
        snap.tps(),
        snap.window.current_window as f64 / crate::types::WINDOW_MAX as f64 * 100.0,
    )
}

/// Render a Prometheus-compatible text exposition.
pub fn format_prometheus(snap: &MetricsSnapshot) -> String {
    format!(
        "# HELP tpu_window_size Current send window size\n\
         # TYPE tpu_window_size gauge\n\
         tpu_window_size {}\n\
         # HELP tpu_in_flight Transactions currently in-flight\n\
         # TYPE tpu_in_flight gauge\n\
         tpu_in_flight {}\n\
         # HELP tpu_rtt_ms Smoothed RTT estimate (ms)\n\
         # TYPE tpu_rtt_ms gauge\n\
         tpu_rtt_ms {:.2}\n\
         # HELP tpu_submitted_total Total submitted transactions\n\
         # TYPE tpu_submitted_total counter\n\
         tpu_submitted_total {}\n\
         # HELP tpu_acked_total Total acknowledged transactions\n\
         # TYPE tpu_acked_total counter\n\
         tpu_acked_total {}\n\
         # HELP tpu_backpressured_total Transactions dropped due to backpressure\n\
         # TYPE tpu_backpressured_total counter\n\
         tpu_backpressured_total {}\n\
         # HELP tpu_tps Throughput (acked/sec)\n\
         # TYPE tpu_tps gauge\n\
         tpu_tps {:.2}\n",
        snap.window.current_window,
        snap.window.in_flight,
        snap.rtt.smoothed_ms,
        snap.stats.submitted,
        snap.stats.acknowledged,
        snap.stats.backpressured,
        snap.tps(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CongestionConfig, CongestionPhase, RttEstimate, WindowState};

    fn dummy_snap() -> MetricsSnapshot {
        MetricsSnapshot {
            window: WindowState {
                current_window: 16,
                in_flight:       4,
                phase:           CongestionPhase::CongestionAvoidance,
                ssthresh:        32,
            },
            rtt: RttEstimate { smoothed_ms: 380.0, latest_ms: 360.0, samples: 10 },
            stats: SendStats {
                submitted:     100,
                acknowledged:  95,
                backpressured: 5,
                reductions:    2,
                increases:     8,
                peak_window:   20,
                window:        None,
                rtt:           RttEstimate::default(),
            },
            elapsed_secs: 10.0,
        }
    }

    #[test]
    fn tps_calculation() {
        let s = dummy_snap();
        assert!((s.tps() - 9.5).abs() < 0.01);
    }

    #[test]
    fn drop_rate() {
        let s = dummy_snap();
        assert!((s.stats.drop_rate() - 0.05).abs() < 0.001);
    }

    #[test]
    fn format_metrics_contains_key_fields() {
        let s = dummy_snap();
        let out = format_metrics(&s);
        assert!(out.contains("congestion-avoidance"));
        assert!(out.contains("16"));
        assert!(out.contains("380.0"));
    }

    #[test]
    fn format_prometheus_valid_lines() {
        let s = dummy_snap();
        let out = format_prometheus(&s);
        assert!(out.contains("tpu_window_size 16"));
        assert!(out.contains("tpu_in_flight 4"));
        assert!(out.contains("tpu_tps"));
    }
}