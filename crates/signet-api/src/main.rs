mod rpc;

use std::net::SocketAddr;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "signet-api", about = "Signet sandbox provisioning API")]
struct Args {
    #[arg(long, env = "SIGNET_API_LISTEN", default_value = "0.0.0.0:8081")]
    listen: SocketAddr,

    #[arg(
        long,
        env = "SIGNET_API_PUBLIC_URL",
        default_value = "http://localhost:8081"
    )]
    public_url: String,

    #[arg(
        long,
        env = "SIGNET_ENV_HOST",
        default_value = "http://localhost",
        help = "Base for per-env explorer URLs: [scheme://]host"
    )]
    env_host: String,

    #[arg(long, env = "DATABASE_URL")]
    database_url: String,

    #[arg(
        long,
        env = "SIGNET_REAPER_INTERVAL_SECS",
        default_value_t = 60,
        help = "Reaper pass interval in seconds; 0 disables the reaper"
    )]
    reaper_interval_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let pool = signet_db::connect(&args.database_url).await?;
    let orchestrator = signet_orchestrator::Orchestrator::connect(&args.env_host)
        .await
        .map_err(|e| anyhow::anyhow!("kube client: {e}"))?;

    if args.reaper_interval_secs > 0 {
        let reaper = orchestrator.clone();
        let interval = std::time::Duration::from_secs(args.reaper_interval_secs);
        tracing::info!(interval_secs = args.reaper_interval_secs, "reaper enabled");
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                if let Err(e) = reaper.reap_expired().await {
                    tracing::warn!(error = %e, "reaper pass failed");
                }
            }
        });
    } else {
        tracing::info!("reaper disabled");
    }

    let app = rpc::router(pool, orchestrator, args.public_url);

    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(listen = %args.listen, "signet-api listening");
    axum::serve(listener, app).await?;
    Ok(())
}
