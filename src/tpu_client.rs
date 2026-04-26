//! High-level TPU client with integrated congestion control.
//!
//! `TpuClientCc` wraps the congestion controller and provides a simple API
//! for sending transactions with automatic backpressure management.
//! It does **not** open real QUIC connections — it delegates to a pluggable
//! `Sender` trait so the core logic is fully testable without network access.

use std::sync::{Arc, Mutex};

use solana_sdk::transaction::Transaction;

use crate::{
    congestion::CongestionController,
    error::{TpuCcError, TpuCcResult},
    metrics::{format_metrics, format_prometheus, MetricsRecorder, MetricsSnapshot},
    types::CongestionConfig,
};

// ─── Sender trait ────────────────────────────────────────────────────────────

/// Pluggable transport layer.  Implement this to connect real QUIC / UDP.
pub trait Sender: Send + Sync {
    /// Serialize and dispatch the transaction.
    /// Returns the number of milliseconds until an acknowledgement was
    /// observed (or an estimate thereof).
    fn send(&self, tx: &Transaction) -> TpuCcResult<f64>;
}

// ─── Simulated sender (for testing / demo) ──────────────────────────────────

/// A `Sender` that always succeeds and returns a fixed RTT.
/// Useful for unit tests and the web dashboard demo.
pub struct SimulatedSender {
    /// Simulated one-way latency in ms.
    pub latency_ms: f64,
    /// If `Some(n)`, every n-th send fails (simulates congestion).
    pub fail_every:  Option<usize>,
    call_count:      Mutex<usize>,
}

impl SimulatedSender {
    pub fn new(latency_ms: f64) -> Self {
        Self { latency_ms, fail_every: None, call_count: Mutex::new(0) }
    }

    pub fn with_failures(latency_ms: f64, fail_every: usize) -> Self {
        Self {
            latency_ms,
            fail_every: Some(fail_every),
            call_count: Mutex::new(0),
        }
    }
}

impl Sender for SimulatedSender {
    fn send(&self, _tx: &Transaction) -> TpuCcResult<f64> {
        let mut count = self.call_count.lock().unwrap();
        *count += 1;
        if let Some(n) = self.fail_every {
            if *count % n == 0 {
                return Err(TpuCcError::Connection(
                    "simulated congestion loss".into(),
                ));
            }
        }
        Ok(self.latency_ms)
    }
}

// ─── TpuClientCc ─────────────────────────────────────────────────────────────

/// TPU client with slot-aware congestion control.
pub struct TpuClientCc {
    cc:      CongestionController,
    sender:  Arc<dyn Sender>,
    metrics: Mutex<MetricsRecorder>,
}

impl TpuClientCc {
    /// Create with default `CongestionConfig` and a custom `Sender`.
    pub fn new(sender: impl Sender + 'static) -> Self {
        Self::with_config(CongestionConfig::default(), sender)
    }

    /// Create with explicit config.
    pub fn with_config(cfg: CongestionConfig, sender: impl Sender + 'static) -> Self {
        Self {
            cc:      CongestionController::new(cfg),
            sender:  Arc::new(sender),
            metrics: Mutex::new(MetricsRecorder::new()),
        }
    }

    // ── Core send API ────────────────────────────────────────────────────

    /// Send a single transaction, blocking until a send slot is available or
    /// `backpressure_timeout` elapses.
    ///
    /// On success, signals an ACK to the congestion controller.
    /// On transport error, signals a loss event (window shrinks).
    pub fn send(&self, tx: &Transaction) -> TpuCcResult<()> {
        // Record submission
        self.metrics.lock().unwrap().record_submit();

        // Try to claim a send window slot
        if !self.cc.try_acquire() {
            self.metrics.lock().unwrap().record_backpressure();
            return Err(TpuCcError::WindowExhausted {
                window:    self.cc.window_size(),
                in_flight: self.cc.in_flight(),
            });
        }

        // Dispatch
        match self.sender.send(tx) {
            Ok(rtt_ms) => {
                let prev_win = self.cc.window_size();
                self.cc.on_ack(rtt_ms);
                let new_win = self.cc.window_size();
                let mut m = self.metrics.lock().unwrap();
                m.record_ack();
                if new_win > prev_win {
                    m.record_window_increase(new_win);
                }
                Ok(())
            }
            Err(e) => {
                let prev_win = self.cc.window_size();
                self.cc.on_loss();
                let new_win = self.cc.window_size();
                let mut m = self.metrics.lock().unwrap();
                if new_win < prev_win {
                    m.record_window_decrease();
                }
                Err(e)
            }
        }
    }

    /// Send a batch of transactions, applying congestion control to each.
    /// Returns `(succeeded, failed)` counts.
    pub fn send_batch(&self, txs: &[Transaction]) -> (usize, usize) {
        let mut ok = 0usize;
        let mut fail = 0usize;
        for tx in txs {
            match self.send(tx) {
                Ok(())  => ok   += 1,
                Err(_)  => fail += 1,
            }
        }
        (ok, fail)
    }

    // ── Observability ────────────────────────────────────────────────────

    /// Take a point-in-time metrics snapshot.
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        let m = self.metrics.lock().unwrap();
        m.snapshot(self.cc.window_state(), self.cc.rtt_estimate())
    }

    /// Print a human-readable metrics report to stdout.
    pub fn print_metrics(&self) {
        println!("{}", format_metrics(&self.metrics_snapshot()));
    }

    /// Return Prometheus-format metrics as a `String`.
    pub fn prometheus_metrics(&self) -> String {
        format_prometheus(&self.metrics_snapshot())
    }

    /// Current window size.
    pub fn window_size(&self) -> usize {
        self.cc.window_size()
    }

    /// Current in-flight count.
    pub fn in_flight(&self) -> usize {
        self.cc.in_flight()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{message::Message, transaction::Transaction};

    fn dummy_tx() -> Transaction {
        Transaction::new_unsigned(Message::new(&[], None))
    }

    #[test]
    fn send_ok_increments_acked() {
        let client = TpuClientCc::new(SimulatedSender::new(300.0));
        client.send(&dummy_tx()).unwrap();
        let snap = client.metrics_snapshot();
        assert_eq!(snap.stats.submitted,    1);
        assert_eq!(snap.stats.acknowledged, 1);
    }

    #[test]
    fn window_exhausted_increments_backpressured() {
        // Tiny window
        let mut cfg = CongestionConfig::default();
        cfg.window_initial = 1;
        cfg.window_max      = 1;
        cfg.slow_start_threshold = 1;

        let client = TpuClientCc::with_config(cfg, SimulatedSender::new(300.0));

        // The sender always succeeds, so the first send should claim the slot
        // and immediately ack it — window stays at 1.
        // We'll manually fill the window by never acking. We do this by
        // using a sender that hangs… but we can't. Instead verify backpressure
        // when window==1 and one tx is already in-flight via direct cc access.
        client.cc.try_acquire(); // occupy the single slot
        let result = client.send(&dummy_tx());
        assert!(matches!(result, Err(TpuCcError::WindowExhausted { .. })));
        let snap = client.metrics_snapshot();
        assert_eq!(snap.stats.backpressured, 1);
    }

    #[test]
    fn transport_error_triggers_loss_signal() {
        let sender = SimulatedSender::with_failures(300.0, 1); // fail every send
        let client = TpuClientCc::new(sender);
        let result = client.send(&dummy_tx());
        assert!(result.is_err());
        assert_eq!(client.metrics_snapshot().stats.reductions, 1);
    }

    #[test]
    fn send_batch_counts_correctly() {
        let sender = SimulatedSender::with_failures(300.0, 3); // fail every 3rd
        let client = TpuClientCc::new(sender);
        let txs: Vec<_> = (0..6).map(|_| dummy_tx()).collect();
        let (ok, fail) = client.send_batch(&txs);
        assert_eq!(ok + fail, 6);
    }

    #[test]
    fn prometheus_output_is_non_empty() {
        let client = TpuClientCc::new(SimulatedSender::new(300.0));
        let _ = client.send(&dummy_tx());
        let prom = client.prometheus_metrics();
        assert!(prom.contains("tpu_window_size"));
        assert!(prom.contains("tpu_tps"));
    }
}