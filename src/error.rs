use thiserror::Error;

#[derive(Debug, Error)]
pub enum TpuCcError {
    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("send window exhausted — backpressure active (window={window}, in_flight={in_flight})")]
    WindowExhausted { window: usize, in_flight: usize },

    #[error("connection to TPU endpoint failed: {0}")]
    Connection(String),

    #[error("transaction serialization failed: {0}")]
    Serialization(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("timeout after {ms}ms")]
    Timeout { ms: u64 },
}

pub type TpuCcResult<T> = Result<T, TpuCcError>;