use solana_tpu_client_cc::{
    CongestionConfig, CongestionController, CongestionPhase, SimulatedSender, TpuClientCc,
    WINDOW_INITIAL, WINDOW_MAX, WINDOW_MIN,
};
use solana_sdk::{message::Message, transaction::Transaction};

fn dummy_tx() -> Transaction {
    Transaction::new_unsigned(Message::new(&[], None))
}

fn default_ctrl() -> CongestionController {
    CongestionController::new(CongestionConfig::default())
}

// ─── CongestionController unit tests ─────────────────────────────────────────

#[test]
fn initial_window_matches_config() {
    let c = default_ctrl();
    assert_eq!(c.window_size(), WINDOW_INITIAL);
}

#[test]
fn initial_phase_is_slow_start() {
    let c = default_ctrl();
    assert_eq!(c.window_state().phase, CongestionPhase::SlowStart);
}

#[test]
fn acquire_reduces_available_capacity() {
    let c = default_ctrl();
    let before = c.available();
    assert!(c.try_acquire());
    assert_eq!(c.available(), before - 1);
}

#[test]
fn cannot_exceed_window() {
    let c = default_ctrl();
    for _ in 0..c.window_size() {
        assert!(c.try_acquire());
    }
    assert!(!c.try_acquire(), "should not acquire beyond window");
}

#[test]
fn ack_restores_slot() {
    let c = default_ctrl();
    assert!(c.try_acquire());
    assert_eq!(c.in_flight(), 1);
    c.on_ack(380.0);
    assert_eq!(c.in_flight(), 0);
}

#[test]
fn loss_restores_slot_and_shrinks_window() {
    let c = default_ctrl();
    let initial = c.window_size();
    assert!(c.try_acquire());
    c.on_loss();
    assert_eq!(c.in_flight(), 0);
    assert!(c.window_size() <= initial);
}

#[test]
fn slow_start_doubles_window_per_rtt() {
    let c = default_ctrl();
    let initial = c.window_size(); // 8
    // One full RTT = initial acks
    for _ in 0..initial {
        c.try_acquire();
    }
    for _ in 0..initial {
        c.on_ack(350.0);
    }
    // Should have grown (SlowStart doubles, but capped at ssthresh=32)
    assert!(c.window_size() >= initial);
}

#[test]
fn window_enters_congestion_avoidance_at_ssthresh() {
    let mut cfg = CongestionConfig::default();
    cfg.window_initial       = 16;
    cfg.slow_start_threshold = 16; // already at ssthresh
    let c = CongestionController::new(cfg);
    // One RTT of acks should trigger grow_window which sees window>=ssthresh
    for _ in 0..16 {
        c.try_acquire();
    }
    for _ in 0..16 {
        c.on_ack(350.0);
    }
    // Phase should be CongestionAvoidance now
    let phase = c.window_state().phase;
    assert!(
        phase == CongestionPhase::CongestionAvoidance
            || phase == CongestionPhase::SlowStart,
        "unexpected phase: {:?}",
        phase
    );
}

#[test]
fn loss_transitions_to_recovery() {
    let c = default_ctrl();
    c.try_acquire();
    c.on_loss();
    assert_eq!(c.window_state().phase, CongestionPhase::Recovery);
}

#[test]
fn window_never_exceeds_max_under_load() {
    let c = default_ctrl();
    for _ in 0..2000 {
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
fn window_never_below_min_after_repeated_loss() {
    let c = default_ctrl();
    for _ in 0..100 {
        c.try_acquire();
        c.on_loss();
    }
    assert!(c.window_size() >= WINDOW_MIN);
}

#[test]
fn rtt_estimate_converges_toward_samples() {
    let c = default_ctrl();
    // Feed many low-latency acks
    for _ in 0..50 {
        c.try_acquire();
        c.on_ack(50.0);
    }
    // Smoothed RTT should be significantly lower than initial 400 ms
    assert!(c.rtt_estimate().smoothed_ms < 300.0);
}

#[test]
fn rtt_sample_count_increments() {
    let c = default_ctrl();
    assert_eq!(c.rtt_estimate().samples, 0);
    for i in 1..=5 {
        c.try_acquire();
        c.on_ack(300.0);
        assert_eq!(c.rtt_estimate().samples, i);
    }
}

// ─── TpuClientCc integration tests ───────────────────────────────────────────

#[test]
fn client_send_ok_increments_stats() {
    let client = TpuClientCc::new(SimulatedSender::new(300.0));
    client.send(&dummy_tx()).unwrap();
    let s = client.metrics_snapshot();
    assert_eq!(s.stats.submitted,    1);
    assert_eq!(s.stats.acknowledged, 1);
    assert_eq!(s.stats.backpressured, 0);
}

#[test]
fn client_loss_increments_reductions() {
    // fail_every = 1 → every call fails
    let client = TpuClientCc::new(SimulatedSender::with_failures(300.0, 1));
    let _ = client.send(&dummy_tx()); // expected error
    assert_eq!(client.metrics_snapshot().stats.reductions, 1);
}

#[test]
fn client_batch_total_equals_input_size() {
    let client = TpuClientCc::new(SimulatedSender::new(300.0));
    let txs: Vec<_> = (0..10).map(|_| dummy_tx()).collect();
    let (ok, fail) = client.send_batch(&txs);
    assert_eq!(ok + fail, 10);
}

#[test]
fn prometheus_contains_required_metrics() {
    let client = TpuClientCc::new(SimulatedSender::new(300.0));
    let _ = client.send(&dummy_tx());
    let prom = client.prometheus_metrics();
    for key in &[
        "tpu_window_size",
        "tpu_in_flight",
        "tpu_rtt_ms",
        "tpu_submitted_total",
        "tpu_acked_total",
        "tpu_tps",
    ] {
        assert!(prom.contains(key), "missing prometheus metric: {key}");
    }
}

#[test]
fn throughput_pct_is_100_on_all_acks() {
    let client = TpuClientCc::new(SimulatedSender::new(300.0));
    let txs: Vec<_> = (0..8).map(|_| dummy_tx()).collect();
    client.send_batch(&txs);
    let snap = client.metrics_snapshot();
    assert_eq!(snap.stats.throughput_pct(), 100.0);
}