use clap::Parser;
use signet_bitcoind::Client;
use signet_faucet::{FaucetState, build_router};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "signet-faucet", about = "Signet environment faucet minter")]
struct Args {
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

    #[arg(long, env = "FAUCET_WALLET", default_value = "faucet")]
    wallet: String,

    #[arg(long, env = "FAUCET_LISTEN", default_value = "0.0.0.0:8080")]
    listen: String,

    #[arg(long, env = "FAUCET_MAX_SATS", default_value_t = 10_000_000_000)]
    max_sats: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let client = Client::new(
        args.bitcoin_rpc_url,
        args.bitcoin_rpc_user,
        args.bitcoin_rpc_password,
    );
    let app = build_router(FaucetState::new(client, args.wallet, args.max_sats));

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(listen = %args.listen, "faucet listening");
    axum::serve(listener, app).await?;
    Ok(())
}
