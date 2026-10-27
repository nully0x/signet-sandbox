mod dashboard;
mod rpc;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

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

    #[arg(
        long,
        env = "SIGNET_MESH_INTERVAL_SECS",
        default_value_t = 60,
        help = "Lightning mesh reconcile interval in seconds; 0 disables it"
    )]
    mesh_interval_secs: u64,

    #[arg(
        long,
        env = "SIGNET_NIP98_MAX_AGE_SECS",
        default_value_t = 300,
        help = "NIP-98 acceptance window in seconds; 0 disables expiry (dev only)"
    )]
    nip98_max_age_secs: u64,
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

    if args.nip98_max_age_secs == 0 {
        tracing::warn!(
            "NIP-98 expiry disabled (SIGNET_NIP98_MAX_AGE_SECS=0): dev only, do not deploy"
        );
    }

    if args.reaper_interval_secs > 0 {
        let reaper = orchestrator.clone();
        let pool = pool.clone();
        let interval = std::time::Duration::from_secs(args.reaper_interval_secs);
        tracing::info!(interval_secs = args.reaper_interval_secs, "reaper enabled");
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                match reaper.reap_expired().await {
                    Ok(namespaces) => {
                        for ns in namespaces {
                            // The row must follow the cluster: a reaped env
                            // must never look startable in the database.
                            let short_id = ns.trim_start_matches("env-");
                            match signet_db::mark_expired_by_short_id(&pool, short_id).await {
                                Ok(1) => {
                                    tracing::info!(namespace = %ns, "marked reaped environment expired")
                                }
                                Ok(_) => {
                                    tracing::warn!(namespace = %ns, "reaped namespace has no matching environment row")
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, namespace = %ns, "expired status update failed")
                                }
                            }
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "reaper pass failed"),
                }
            }
        });
    } else {
        tracing::info!("reaper disabled");
    }

    // Envs are isolated chains: public signet seeds never answer, so
    // sibling lightning instances are each other's only peers. The
    // provisioning-time mesh is one-shot; this pass re-runs it so a
    // restarted pod (or a slow wallet) regains its peers.
    if args.mesh_interval_secs > 0 {
        let mesh = orchestrator.clone();
        let pool = pool.clone();
        let interval = std::time::Duration::from_secs(args.mesh_interval_secs);
        tracing::info!(
            interval_secs = args.mesh_interval_secs,
            "lightning mesh reconciler enabled"
        );
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let targets = match signet_db::lightning_mesh_targets(&pool).await {
                    Ok(targets) => targets,
                    Err(e) => {
                        tracing::warn!(error = %e, "mesh target lookup failed");
                        continue;
                    }
                };
                let mut envs: BTreeMap<Uuid, Vec<String>> = BTreeMap::new();
                for (env_id, name) in targets {
                    envs.entry(env_id).or_default().push(name);
                }
                for (env_id, names) in envs {
                    if names.len() < 2 {
                        continue;
                    }
                    let short = env_id.simple().to_string()[..12].to_string();
                    match mesh.mesh_lightning_peers(&short, &names).await {
                        Ok(0) => {}
                        Ok(initiated) => {
                            tracing::info!(env = %short, initiated, "lightning instances peered")
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, env = %short, "mesh reconcile skipped")
                        }
                    }
                }
            }
        });
    } else {
        tracing::info!("lightning mesh reconciler disabled");
    }

    let catalog = signet_core::versions::VersionCatalog::embedded();
    let app = rpc::router(
        pool,
        orchestrator,
        catalog,
        args.public_url,
        if args.nip98_max_age_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(args.nip98_max_age_secs))
        },
    );

    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(listen = %args.listen, "signet-api listening");
    axum::serve(listener, app).await?;
    Ok(())
}
