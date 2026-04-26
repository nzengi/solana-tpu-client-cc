//! Basic usage example — demonstrates congestion-controlled batch sending
//! using the built-in `SimulatedSender` (no real network required).

use solana_sdk::{message::Message, transaction::Transaction};
use solana_tpu_client_cc::{CongestionConfig, SimulatedSender, TpuClientCc};

fn dummy_tx() -> Transaction {
    Transaction::new_unsigned(Message::new(&[], None))
}

fn main() {
    println!("=== solana-tpu-client-cc basic example ===\n");

    // ── 1. Default config (good for most cases) ──────────────────────────
    let client = TpuClientCc::new(SimulatedSender::new(350.0));

    println!("Sending 20 transactions with default config…");
    let txs: Vec<_> = (0..20).map(|_| dummy_tx()).collect();
    let (ok, fail) = client.send_batch(&txs);
    println!("  ok={ok}  failed={fail}");
    client.print_metrics();

    // ── 2. Simulate congestion (every 5th tx fails) ────────────────────
    println!("\nSimulating congestion (1-in-5 loss rate)…");
    let lossy = TpuClientCc::new(SimulatedSender::with_failures(400.0, 5));
    let txs2: Vec<_> = (0..30).map(|_| dummy_tx()).collect();
    let (ok2, fail2) = lossy.send_batch(&txs2);
    println!("  ok={ok2}  failed={fail2}");
    lossy.print_metrics();

    // ── 3. Custom config — small initial window, aggressive β ──────────
    println!("\nCustom config (window_initial=2, β=0.7)…");
    let mut cfg = CongestionConfig::default();
    cfg.window_initial  = 2;
    cfg.beta_decrease   = 0.7;
    let conservative = TpuClientCc::with_config(cfg, SimulatedSender::new(300.0));
    let txs3: Vec<_> = (0..15).map(|_| dummy_tx()).collect();
    let (ok3, fail3) = conservative.send_batch(&txs3);
    println!("  ok={ok3}  failed={fail3}");
    conservative.print_metrics();

    // ── 4. Prometheus metrics ─────────────────────────────────────────
    println!("\n--- Prometheus exposition (first client) ---");
    print!("{}", client.prometheus_metrics());
}