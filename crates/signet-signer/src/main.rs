mod signet;

use std::time::Duration;

use anyhow::{Context, anyhow};
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use bitcoin::{Network, PrivateKey, ScriptBuf};
use clap::{Parser, Subcommand};
use serde_json::json;
use tracing_subscriber::EnvFilter;

use signet_bitcoind::Client;

#[derive(Parser, Debug)]
#[command(name = "signet-signer", about = "Signet block signing service")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Generate a signer keypair and print the challenge
    Keygen,
    /// Run the block signing loop
    Run(RunArgs),
}

#[derive(Parser, Debug, Clone)]
struct RunArgs {
    #[arg(
        long,
        env = "BITCOIN_RPC_URL",
        default_value = "http://127.0.0.1:38332"
    )]
    bitcoin_rpc_url: String,

    #[arg(long, env = "BITCOIN_RPC_USER", default_value = "signet")]
    bitcoin_rpc_user: String,

    #[arg(long, env = "BITCOIN_RPC_PASSWORD", default_value = "signet")]
    bitcoin_rpc_password: String,

    #[arg(long, env = "SIGNER_KEY_WIF")]
    key_wif: String,

    #[arg(long, env = "SIGNER_INTERVAL_SECS", default_value_t = 30)]
    interval_secs: u64,

    #[arg(long, env = "SIGNER_PREMINE_BLOCKS", default_value_t = 101)]
    premine_blocks: u32,

    #[arg(long, env = "SIGNER_WALLET", default_value = "faucet")]
    wallet: String,
}

fn keygen() -> anyhow::Result<()> {
    let secp = Secp256k1::new();
    let key = SecretKey::new(&mut bitcoin::secp256k1::rand::thread_rng());
    let pubkey = key.public_key(&secp);
    let challenge = signet::challenge_for_pubkey(&pubkey);
    let wif = PrivateKey::new(key, Network::Testnet).to_wif();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "signer_privkey_wif": wif,
            "signer_pubkey": pubkey.to_string(),
            "signet_challenge": challenge.to_hex_string(),
        }))?
    );
    Ok(())
}

async fn ensure_wallet(client: &Client, wallet: &str) -> anyhow::Result<ScriptBuf> {
    let wallet_client = client.wallet(wallet);
    if wallet_client.get_wallet_info().await.is_err() {
        client
            .create_wallet(wallet)
            .await
            .with_context(|| format!("creating wallet {wallet}"))?;
    }
    let address = wallet_client.get_new_address("coinbase").await?;
    let info = wallet_client.get_address_info(&address).await?;
    let spk = ScriptBuf::from_hex(&info.script_pub_key)
        .map_err(|e| anyhow!("bad scriptPubKey from node: {e}"))?;
    tracing::info!(%address, "coinbase reward address");
    Ok(spk)
}

async fn mine_once(
    client: &Client,
    reward_spk: &bitcoin::Script,
    key: &SecretKey,
    expected_challenge: &bitcoin::Script,
) -> anyhow::Result<bitcoin::BlockHash> {
    let tmpl = client.get_block_template().await?;
    let block = signet::assemble_block(&tmpl, reward_spk, key, Some(expected_challenge))?;
    let hash = block.block_hash();
    let hex = bitcoin::consensus::encode::serialize_hex(&block);
    match client.submit_block(&hex).await? {
        None => Ok(hash),
        Some(reason) => Err(anyhow!("submitblock rejected: {reason}")),
    }
}

async fn run(args: RunArgs) -> anyhow::Result<()> {
    let key = PrivateKey::from_wif(&args.key_wif)
        .map_err(|e| anyhow!("invalid SIGNER_KEY_WIF: {e}"))?
        .inner;
    let secp = Secp256k1::new();
    let expected_challenge = signet::challenge_for_pubkey(&key.public_key(&secp));
    tracing::info!(challenge = %expected_challenge.to_hex_string(), "signer key loaded");

    let client = Client::new(
        args.bitcoin_rpc_url.clone(),
        args.bitcoin_rpc_user,
        args.bitcoin_rpc_password,
    );

    let mut reward_spk: Option<ScriptBuf> = None;
    let mut interval = tokio::time::interval(Duration::from_secs(args.interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        interval.tick().await;

        let info = match client.get_blockchain_info().await {
            Ok(info) => info,
            Err(e) => {
                tracing::warn!(error = %e, "bitcoind not reachable yet");
                continue;
            }
        };

        if reward_spk.is_none() {
            match ensure_wallet(&client, &args.wallet).await {
                Ok(spk) => reward_spk = Some(spk),
                Err(e) => {
                    tracing::warn!(error = %e, "wallet not ready yet");
                    continue;
                }
            }
        }
        let spk = reward_spk.as_ref().expect("set above");

        match mine_once(&client, spk, &key, &expected_challenge).await {
            Ok(hash) => {
                tracing::info!(height = info.blocks + 1, %hash, "block accepted");
            }
            Err(e) => {
                tracing::warn!(error = %e, "mining attempt failed");
                continue;
            }
        }

        if info.blocks + 1 < args.premine_blocks {
            interval.reset_after(Duration::from_millis(250));
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    match args.command {
        None | Some(Command::Run(_)) => {
            let run_args = match args.command {
                Some(Command::Run(r)) => r,
                _ => RunArgs::parse(),
            };
            run(run_args).await
        }
        Some(Command::Keygen) => keygen(),
    }
}
