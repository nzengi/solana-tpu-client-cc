//! Slot-aware congestion controller.
//!
//! Implements an AIMD (Additive Increase / Multiplicative Decrease) algorithm
//! calibrated to Solana's ~400 ms slot boundary instead of TCP's ACK clock.
//!
//! State machine:
//!   SlowStart → CongestionAvoidance (when window ≥ ssthresh)
//!   CongestionAvoidance → Recovery   (on backpressure signal)
//!   Recovery            → CongestionAvoidance (after one clean RTT)

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::types::{CongestionConfig, CongestionPhase, RttEstimate, WindowState, WINDOW_MAX, WINDOW_MIN};

// ─── Internal state ─────────────────────────────────────────────────────────

#[derive(Debug)]
struct Inner {
    cfg:   CongestionConfig,
    win:   WindowState,
    rtt:   RttEstimate,
    /// Timestamp of the last successful ack — used to pace window growth.
    last_ack:    Option<Instant>,
    /// How many successful acks have accumulated since the last window increase.
    ack_pending: usize,
}

impl Inner {
    fn new(cfg: CongestionConfig) -> Self {
        let win = WindowState::new(&cfg);
        Self {
            cfg,
            win,
            rtt: RttEstimate::default(),
            last_ack: None,
            ack_pending: 0,
        }
    }

    // ── Capacity ──────────────────────────────────────────────────────────

    fn available(&self) -> usize {
        self.win.available()
    }

    // ── Acquire a send slot ───────────────────────────────────────────────

    /// Attempt to consume one slot from the send window.
    /// Returns `true` if a slot was available.
    fn try_acquire(&mut self) -> bool {
        if self.win.in_flight < self.win.current_window {
            self.win.in_flight += 1;
            true
        } else {
            false
        }
    }

    // ── Release a send slot (ACK path) ────────────────────────────────────

    fn on_ack(&mut self, rtt_sample_ms: f64) {
        // Release in-flight counter
        self.win.in_flight = self.win.in_flight.saturating_sub(1);

        // Update RTT estimate
        self.rtt.update(rtt_sample_ms, self.cfg.rtt_alpha);
        self.last_ack = Some(Instant::now());
        self.ack_pending += 1;

        // Window growth — only increase once per RTT cycle
        // (ack_pending must reach current_window to count as one full RTT)
        let rtt_complete = self.ack_pending >= self.win.current_window.max(1);
        if rtt_complete {
            self.ack_pending = 0;
            self.grow_window();
        }

        // Leave Recovery after one clean RTT
        if self.win.phase == CongestionPhase::Recovery && rtt_complete {
            self.win.phase = CongestionPhase::CongestionAvoidance;
        }
    }

    // ── Release a send slot (loss / backpressure path) ────────────────────

    fn on_loss(&mut self) {
        self.win.in_flight = self.win.in_flight.saturating_sub(1);
        self.shrink_window();
        self.ack_pending = 0;
    }

    // ── Window growth (AIMD increase) ─────────────────────────────────────

    fn grow_window(&mut self) {
        match self.win.phase {
            CongestionPhase::SlowStart => {
                // Double each RTT until ssthresh
                let next = (self.win.current_window * 2).min(self.win.ssthresh).min(self.cfg.window_max);
                self.win.current_window = next;
                if self.win.current_window >= self.win.ssthresh {
                    self.win.phase = CongestionPhase::CongestionAvoidance;
                }
            }
            CongestionPhase::CongestionAvoidance | CongestionPhase::Recovery => {
                // Additive increase
                let next = (self.win.current_window + self.cfg.additive_increase)
                    .min(self.cfg.window_max)
                    .min(WINDOW_MAX);
                self.win.current_window = next;
            }
        }
    }

    // ── Window shrink (AIMD decrease) ─────────────────────────────────────

    fn shrink_window(&mut self) {
        // Update ssthresh = max(window/2, window_min)
        self.win.ssthresh = (self.win.current_window / 2).max(self.cfg.window_min).max(WINDOW_MIN);
        // New window = max(ssthresh, window_min)
        self.win.current_window = self.win.ssthresh.max(self.cfg.window_min);
        self.win.phase = CongestionPhase::Recovery;
    }

    // ── Snapshot ──────────────────────────────────────────────────────────

    fn window_snapshot(&self) -> WindowState {
        self.win.clone()
    }

    fn rtt_snapshot(&self) -> RttEstimate {
        self.rtt.clone()
    }
}

// ─── Public handle ───────────────────────────────────────────────────────────

/// Thread-safe congestion controller handle.
///
/// Clone freely — all clones share the same inner state.
#[derive(Debug, Clone)]
pub struct CongestionController(Arc<Mutex<Inner>>);

impl CongestionController {
    pub fn new(cfg: CongestionConfig) -> Self {
        Self(Arc::new(Mutex::new(Inner::new(cfg))))
    }

    /// Number of send slots available right now.
    pub fn available(&self) -> usize {
        self.0.lock().unwrap().available()
    }

    /// Try to claim one send slot.  Returns `true` if the slot was granted.
    pub fn try_acquire(&self) -> bool {
        self.0.lock().unwrap().try_acquire()
    }

    /// Signal a successful delivery with its measured RTT in milliseconds.
    pub fn on_ack(&self, rtt_ms: f64) {
        self.0.lock().unwrap().on_ack(rtt_ms);
    }

    /// Signal backpressure or packet loss — shrinks the window.
    pub fn on_loss(&self) {
        self.0.lock().unwrap().on_loss();
    }

    /// Snapshot of the current window state (for metrics / display).
    pub fn window_state(&self) -> WindowState {
        self.0.lock().unwrap().window_snapshot()
    }

    /// Snapshot of the current RTT estimate.
    pub fn rtt_estimate(&self) -> RttEstimate {
        self.0.lock().unwrap().rtt_snapshot()
    }

    /// Current send window size.
    pub fn window_size(&self) -> usize {
        self.0.lock().unwrap().win.current_window
    }

    /// Current in-flight count.
    pub fn in_flight(&self) -> usize {
        self.0.lock().unwrap().win.in_flight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl() -> CongestionController {
        CongestionController::new(CongestionConfig::default())
    }

    #[test]
    fn initial_window_is_configured_value() {
        let c = ctrl();
        assert_eq!(c.window_size(), crate::types::WINDOW_INITIAL);
    }

    #[test]
    fn acquire_decrements_available() {
        let c = ctrl();
        let avail_before = c.available();
        assert!(c.try_acquire());
        assert_eq!(c.available(), avail_before - 1);
    }

    #[test]
    fn window_exhausted_returns_false() {
        let c = ctrl();
        // Fill up the window
        for _ in 0..c.window_size() {
            assert!(c.try_acquire());
        }
        assert!(!c.try_acquire());
    }

    #[test]
    fn ack_releases_slot_and_grows_window() {
        let c = ctrl();
        let initial = c.window_size();
        // Acquire all slots, then ack all — should trigger one RTT cycle
        for _ in 0..initial {
            c.try_acquire();
        }
        for _ in 0..initial {
            c.on_ack(350.0);
        }
        // After one RTT in SlowStart, window should double (capped at ssthresh)
        assert!(c.window_size() >= initial);
        assert_eq!(c.in_flight(), 0);
    }

    #[test]
    fn loss_shrinks_window() {
        let c = ctrl();
        let initial = c.window_size();
        c.try_acquire();
        c.on_loss();
        assert!(c.window_size() <= initial);
    }

    #[test]
    fn window_never_exceeds_max() {
        let c = ctrl();
        // Simulate many ack cycles
        for _ in 0..1000 {
            let avail = c.available();
            for _ in 0..avail {
                c.try_acquire();
            }
            for _ in 0..avail.max(1) {
                c.on_ack(100.0);
            }
        }
        assert!(c.window_size() <= WINDOW_MAX);
    }

    #[test]
    fn window_never_below_min() {
        let c = ctrl();
        for _ in 0..50 {
            c.try_acquire();
            c.on_loss();
        }
        assert!(c.window_size() >= WINDOW_MIN);
    }

    #[test]
    fn rtt_estimate_updates() {
        let c = ctrl();
        let initial_rtt = c.rtt_estimate().smoothed_ms;
        c.try_acquire();
        c.on_ack(100.0); // much lower than default 400ms
        let updated_rtt = c.rtt_estimate().smoothed_ms;
        assert!(updated_rtt < initial_rtt);
        assert_eq!(c.rtt_estimate().samples, 1);
    }
}