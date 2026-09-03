mod rpc;

use std::net::SocketAddr;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "signet-api", about = "Signet sandbox provisioning API")]
struct Args {
    #[arg(long, env = "SIGNET_API_LISTEN", default_value = "0.0.0.0:8080")]
    listen: SocketAddr,

    #[arg(
        long,
        env = "SIGNET_API_PUBLIC_URL",
        default_value = "http://localhost:8080"
    )]
    public_url: String,

    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
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
    let orchestrator = signet_orchestrator::Orchestrator::connect()
        .await
        .map_err(|e| anyhow::anyhow!("kube client: {e}"))?;

    let app = rpc::router(pool, orchestrator, args.public_url);

    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(listen = %args.listen, "signet-api listening");
    axum::serve(listener, app).await?;
    Ok(())
}
