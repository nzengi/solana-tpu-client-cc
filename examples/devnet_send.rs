//! Devnet integration example — sends real SOL transfers via RpcSender.
//!
//! Prerequisites:
//!   solana config set --url devnet
//!   solana airdrop 1
//!
//! Run:
//!   cargo run --example devnet_send
//!   cargo run --example devnet_send -- --rpc https://api.mainnet-beta.solana.com

use std::{path::PathBuf, time::Instant};

use solana_rpc_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    signature::{read_keypair_file, Keypair},
    signer::Signer,
    system_instruction,
    transaction::Transaction,
};
use solana_tpu_client_cc::{CongestionConfig, RpcSender, TpuClientCc};

const DEFAULT_RPC: &str = "https://api.devnet.solana.com";
const BATCH_SIZE: usize = 10;
const TRANSFER_LAMPORTS: u64 = 5_000; // 0.000005 SOL per tx

fn load_payer() -> Keypair {
    let default_path = PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/solana/id.json");
    if let Ok(kp) = read_keypair_file(&default_path) {
        eprintln!("Keypair: {}", default_path.display());
        kp
    } else {
        panic!(
            "No keypair at ~/.config/solana/id.json\n\
             Run: solana-keygen new && solana airdrop 1 --url devnet"
        );
    }
}

fn main() -> anyhow::Result<()> {
    // ── Parse optional --rpc flag ─────────────────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    let rpc_url = args
        .windows(2)
        .find(|w| w[0] == "--rpc")
        .map(|w| w[1].as_str())
        .unwrap_or(DEFAULT_RPC)
        .to_string();

    println!("=== solana-tpu-client-cc devnet example ===");
    println!("RPC: {rpc_url}\n");

    // ── Keypair & balance check ───────────────────────────────────────────
    let payer = load_payer();
    println!("Payer: {}", payer.pubkey());

    let plain_rpc = RpcClient::new_with_commitment(
        rpc_url.clone(),
        CommitmentConfig::confirmed(),
    );

    let balance = plain_rpc.get_balance(&payer.pubkey())?;
    println!(
        "Balance: {} lamports ({:.6} SOL)\n",
        balance,
        balance as f64 / 1e9
    );

    let min_required = TRANSFER_LAMPORTS * BATCH_SIZE as u64 + 100_000; // + fee reserve
    if balance < min_required {
        anyhow::bail!(
            "Insufficient balance ({balance} lamports). Need at least {min_required}.\n\
             Run: solana airdrop 1 --url devnet"
        );
    }

    // ── Build TpuClientCc with RpcSender ─────────────────────────────────
    let cfg = CongestionConfig {
        window_initial: 4,
        window_max: 32,
        ..CongestionConfig::default()
    };
    let sender = RpcSender::new(&rpc_url);
    let client = TpuClientCc::with_config(cfg, sender);

    println!("Sending {BATCH_SIZE} transactions (congestion control active)…\n");

    // ── Build and send transactions one by one, measuring per-tx latency ─
    let mut signatures = Vec::with_capacity(BATCH_SIZE);
    let mut rtts_ms: Vec<f64> = Vec::with_capacity(BATCH_SIZE);
    let batch_start = Instant::now();

    for i in 0..BATCH_SIZE {
        // Each tx sends to a fresh throwaway address so they're all unique
        let recipient = Keypair::new().pubkey();
        let blockhash = plain_rpc.get_latest_blockhash()?;

        let ix = system_instruction::transfer(&payer.pubkey(), &recipient, TRANSFER_LAMPORTS);
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );

        let t0 = Instant::now();
        match client.send(&tx) {
            Ok(()) => {
                let elapsed = t0.elapsed().as_secs_f64() * 1_000.0;
                rtts_ms.push(elapsed);
                // Extract signature from the signed tx
                let sig = tx.signatures[0];
                println!(
                    "  [{:02}/{BATCH_SIZE}] ✓  sig={:.12}…  rtt={:.1}ms  window={}",
                    i + 1,
                    sig.to_string(),
                    elapsed,
                    client.window_size(),
                );
                signatures.push(sig);
            }
            Err(e) => {
                println!(
                    "  [{:02}/{BATCH_SIZE}] ✗  backpressure/error: {e}  window={}",
                    i + 1,
                    client.window_size(),
                );
            }
        }
    }

    let total_ms = batch_start.elapsed().as_secs_f64() * 1_000.0;

    // ── Summary ───────────────────────────────────────────────────────────
    println!("\n=== Batch complete in {:.1}ms ===", total_ms);
    println!("Sent: {}/{BATCH_SIZE}", signatures.len());

    if !rtts_ms.is_empty() {
        let avg = rtts_ms.iter().sum::<f64>() / rtts_ms.len() as f64;
        let min = rtts_ms.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = rtts_ms.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        println!("RTT  — avg:{avg:.1}ms  min:{min:.1}ms  max:{max:.1}ms");
    }

    println!("\n--- Congestion controller metrics ---");
    client.print_metrics();

    // ── Explorer links ────────────────────────────────────────────────────
    let cluster = if rpc_url.contains("mainnet") { "" } else { "?cluster=devnet" };
    println!("\n--- Explorer links ---");
    for sig in &signatures {
        println!(
            "https://explorer.solana.com/tx/{sig}{cluster}"
        );
    }

    Ok(())
}