use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Solana slot duration — used as the base RTT reference.
pub const SLOT_DURATION_MS: u64 = 400;

/// Hard minimum / maximum for the send window (transactions in-flight).
pub const WINDOW_MIN: usize = 1;
pub const WINDOW_MAX: usize = 512;

/// Default initial window size.
pub const WINDOW_INITIAL: usize = 8;

// ─── Configuration ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CongestionConfig {
    /// Initial send window (number of transactions allowed in-flight).
    pub window_initial: usize,

    /// Minimum window size — never drop below this.
    pub window_min: usize,

    /// Maximum window size — never grow above this.
    pub window_max: usize,

    /// Multiplicative decrease factor on backpressure / loss signal (0 < x < 1).
    pub beta_decrease: f64,

    /// Additive increase per RTT cycle (slots).
    pub additive_increase: usize,

    /// Number of consecutive successful sends before entering slow-start exit.
    pub slow_start_threshold: usize,

    /// Maximum time to wait for in-flight capacity before returning
    /// `WindowExhausted`.
    pub backpressure_timeout: Duration,

    /// How long to smooth RTT estimates (EWMA alpha, 0..1).
    pub rtt_alpha: f64,
}

impl Default for CongestionConfig {
    fn default() -> Self {
        Self {
            window_initial:       WINDOW_INITIAL,
            window_min:           WINDOW_MIN,
            window_max:           WINDOW_MAX,
            beta_decrease:        0.5,
            additive_increase:    1,
            slow_start_threshold: 32,
            backpressure_timeout: Duration::from_millis(SLOT_DURATION_MS * 2),
            rtt_alpha:            0.125,
        }
    }
}

// ─── Window state ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CongestionPhase {
    /// Exponential growth until `slow_start_threshold`.
    SlowStart,
    /// Additive increase / multiplicative decrease.
    CongestionAvoidance,
    /// Window was reduced due to backpressure — recovering.
    Recovery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub current_window: usize,
    pub in_flight:      usize,
    pub phase:          CongestionPhase,
    pub ssthresh:       usize,
}

impl WindowState {
    pub fn new(cfg: &CongestionConfig) -> Self {
        Self {
            current_window: cfg.window_initial,
            in_flight:      0,
            phase:          CongestionPhase::SlowStart,
            ssthresh:       cfg.slow_start_threshold,
        }
    }

    /// Available capacity (window - in_flight).
    pub fn available(&self) -> usize {
        self.current_window.saturating_sub(self.in_flight)
    }
}

// ─── RTT estimate ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RttEstimate {
    /// Smoothed RTT in milliseconds (EWMA).
    pub smoothed_ms: f64,
    /// Latest single-sample RTT.
    pub latest_ms:   f64,
    /// Number of samples observed.
    pub samples:     u64,
}

impl Default for RttEstimate {
    fn default() -> Self {
        Self {
            smoothed_ms: SLOT_DURATION_MS as f64,
            latest_ms:   SLOT_DURATION_MS as f64,
            samples:     0,
        }
    }
}

impl RttEstimate {
    pub fn update(&mut self, sample_ms: f64, alpha: f64) {
        self.latest_ms   = sample_ms;
        self.smoothed_ms = alpha * sample_ms + (1.0 - alpha) * self.smoothed_ms;
        self.samples    += 1;
    }

    /// How many slots does the current RTT span?
    pub fn slots(&self) -> f64 {
        self.smoothed_ms / SLOT_DURATION_MS as f64
    }
}

// ─── Per-send statistics ────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SendStats {
    /// Total transactions submitted to the send window.
    pub submitted:      u64,
    /// Transactions acknowledged (slot confirmation received).
    pub acknowledged:   u64,
    /// Transactions dropped due to backpressure (window exhausted).
    pub backpressured:  u64,
    /// Window reductions triggered.
    pub reductions:     u64,
    /// Window increases.
    pub increases:      u64,
    /// Peak window size observed.
    pub peak_window:    usize,
    /// Current window state snapshot.
    pub window:         Option<WindowState>,
    /// Current RTT estimate.
    pub rtt:            RttEstimate,
}

impl SendStats {
    pub fn drop_rate(&self) -> f64 {
        if self.submitted == 0 {
            return 0.0;
        }
        self.backpressured as f64 / self.submitted as f64
    }

    pub fn throughput_pct(&self) -> f64 {
        if self.submitted == 0 {
            return 0.0;
        }
        self.acknowledged as f64 / self.submitted as f64 * 100.0
    }
}