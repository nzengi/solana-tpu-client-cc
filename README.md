# solana-tpu-client-cc

[![CI](https://github.com/nzengi/solana-tpu-client-cc/actions/workflows/ci.yml/badge.svg)](https://github.com/nzengi/solana-tpu-client-cc/actions)
[![Crates.io](https://img.shields.io/crates/v/solana-tpu-client-cc.svg)](https://crates.io/crates/solana-tpu-client-cc)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Slot-aware AIMD congestion control for Solana TPU clients.**

Solana's official `QuicClient` uses fire-and-forget semantics — it sends transactions as fast as the caller pushes them, with no feedback loop.  Under validator load this causes burst drops, unnecessary retries, and degraded finality rates.  `solana-tpu-client-cc` wraps the transport layer with a send-window controller that adapts to the network in real time, calibrated to Solana's ~400 ms slot boundary instead of TCP's ACK clock.

---

## Why it matters

| Problem | This library |
|---|---|
| QUIC client has no congestion signal | AIMD window shrinks on loss, grows on ACK |
| Burst sends overwhelm validator queues | Send window caps in-flight tx count |
| No visibility into send health | Prometheus + human-readable metrics |
| Hard to test without a live network | Pluggable `Sender` trait + `SimulatedSender` |

---

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│  TpuClientCc                                             │
│                                                          │
│  send(tx) ──► CongestionController ──► Sender trait      │
│                    │   ▲                  │              │
│               try_acquire()           on_ack(rtt_ms)     │
│                    │                  on_loss()           │
│               WindowState ◄──── RTT estimate (EWMA)      │
│                                                          │
│  MetricsRecorder ──► format_metrics() / Prometheus       │
└──────────────────────────────────────────────────────────┘
```

### Congestion state machine

```
SlowStart ──(window ≥ ssthresh)──► CongestionAvoidance
    ▲                                       │
    │                               (loss / backpressure)
    │                                       ▼
    └──────────────(one clean RTT)──── Recovery
```

- **SlowStart**: window doubles every RTT until `ssthresh`.
- **CongestionAvoidance**: window grows +1 per RTT (additive increase).
- **Recovery**: window halved on loss (multiplicative decrease); reverts to `CongestionAvoidance` after one clean RTT.
- RTT is estimated with EWMA (α = 0.125) seeded at one slot duration (400 ms).

---

## Quick start

```toml
# Cargo.toml
[dependencies]
solana-tpu-client-cc = "0.1"
```

```rust
use solana_tpu_client_cc::{TpuClientCc, SimulatedSender, CongestionConfig};
use solana_sdk::{message::Message, transaction::Transaction};

// Drop-in simulated sender (no network needed for testing)
let client = TpuClientCc::new(SimulatedSender::new(350.0));

let tx = Transaction::new_unsigned(Message::new(&[], None));
match client.send(&tx) {
    Ok(())  => println!("sent  window={}", client.window_size()),
    Err(e)  => eprintln!("backpressure: {e}"),
}
client.print_metrics();
```

### Batch send

```rust
let txs: Vec<Transaction> = build_batch();
let (ok, failed) = client.send_batch(&txs);
println!("ok={ok} failed={failed}");
```

### Custom config

```rust
let mut cfg = CongestionConfig::default();
cfg.window_initial       = 4;
cfg.beta_decrease        = 0.7;   // more aggressive shrink
cfg.slow_start_threshold = 16;
let client = TpuClientCc::with_config(cfg, my_quic_sender);
```

### Prometheus metrics

```rust
println!("{}", client.prometheus_metrics());
// tpu_window_size 32
// tpu_in_flight 5
// tpu_rtt_ms 387.50
// tpu_submitted_total 1024
// tpu_acked_total 1012
// tpu_backpressured_total 12
// tpu_tps 84.33
```

---

## Pluggable transport

Implement the `Sender` trait to connect a real QUIC / UDP transport:

```rust
use solana_tpu_client_cc::{Sender, TpuCcResult};
use solana_sdk::transaction::Transaction;

struct MyQuicSender { /* … */ }

impl Sender for MyQuicSender {
    fn send(&self, tx: &Transaction) -> TpuCcResult<f64> {
        let t0 = std::time::Instant::now();
        self.quic_conn.send_transaction(tx)?;
        Ok(t0.elapsed().as_secs_f64() * 1000.0)
    }
}

let client = TpuClientCc::new(MyQuicSender { /* … */ });
```

---

## Configuration reference

| Field | Default | Description |
|---|---|---|
| `window_initial` | 8 | Starting send-window size |
| `window_min` | 1 | Hard floor |
| `window_max` | 512 | Hard ceiling |
| `beta_decrease` | 0.5 | Multiplicative decrease factor on loss |
| `additive_increase` | 1 | Window growth per RTT in `CongestionAvoidance` |
| `slow_start_threshold` | 32 | Window at which slow-start exits |
| `backpressure_timeout` | 800 ms | How long to wait before returning `WindowExhausted` |
| `rtt_alpha` | 0.125 | EWMA smoothing factor for RTT |

---

## Live dashboard

The interactive congestion-control simulator is deployed at  
**[https://solana-tpu-client-cc.vercel.app](https://solana-tpu-client-cc.vercel.app)**

Adjust loss rate and base RTT in real time; watch window size, phase transitions, and RTT converge.

---

## Running tests

```bash
cargo test --lib --tests
```

35 tests, zero network access required.

## Running the example

```bash
cargo run --example basic_send
```

---

## Relation to other projects

| Repo | Role |
|---|---|
| [`solana-tx-retry`](https://github.com/nzengi/solana-tx-retry) | Smart retry + leader-aware routing |
| [`solana-cu-estimator`](https://github.com/nzengi/solana-cu-estimator) | Compute-unit budget estimation |
| **`solana-tpu-client-cc`** | Congestion-controlled send layer |

Together they form a complete transaction lifecycle optimization stack, suitable for a Solana Foundation grant application under the **Network Performance** category.

---

## License

MIT © [nzengi](https://github.com/nzengi)