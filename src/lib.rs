//! # solana-tpu-client-cc
//!
//! Slot-aware congestion control for Solana TPU clients.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use solana_tpu_client_cc::{TpuClientCc, SimulatedSender, CongestionConfig};
//! use solana_sdk::{message::Message, transaction::Transaction};
//!
//! let client = TpuClientCc::new(SimulatedSender::new(350.0));
//! let tx = Transaction::new_unsigned(Message::new(&[], None));
//!
//! match client.send(&tx) {
//!     Ok(())  => println!("sent — window={}", client.window_size()),
//!     Err(e)  => eprintln!("backpressure: {e}"),
//! }
//! client.print_metrics();
//! ```

pub mod congestion;
pub mod error;
pub mod metrics;
pub mod tpu_client;
pub mod types;

// ─── Re-exports ──────────────────────────────────────────────────────────────

pub use congestion::CongestionController;
pub use error::{TpuCcError, TpuCcResult};
pub use metrics::{format_metrics, format_prometheus, MetricsRecorder, MetricsSnapshot};
pub use tpu_client::{RpcSender, Sender, SimulatedSender, TpuClientCc};
pub use types::{
    CongestionConfig, CongestionPhase, RttEstimate, SendStats, WindowState,
    SLOT_DURATION_MS, WINDOW_INITIAL, WINDOW_MAX, WINDOW_MIN,
};