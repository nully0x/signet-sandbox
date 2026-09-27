use signet_orchestrator::{EnvComponents, EnvSecrets, Orchestrator};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let action = args
        .next()
        .expect("usage: provision <create|destroy|ready> <env-id> [faucet|indexer|explorer]");
    let env_id = args.next().expect("missing env id");

    let env_host = std::env::var("SIGNET_ENV_HOST").unwrap_or_else(|_| "localhost".into());
    let orchestrator = Orchestrator::connect(&env_host).await?;
    match action.as_str() {
        "create" => {
            let mut ttl_secs: Option<i64> = None;
            let mut flags: Vec<String> = Vec::new();
            let mut rest = args;
            while let Some(arg) = rest.next() {
                if arg == "--ttl" {
                    let secs = rest.next().expect("--ttl needs a value in seconds");
                    ttl_secs = Some(secs.parse().expect("--ttl must be an integer"));
                } else {
                    flags.push(arg);
                }
            }
            let components = EnvComponents {
                indexer: flags.iter().any(|f| f == "indexer"),
                faucet: flags.iter().any(|f| f == "faucet"),
                explorer: flags.iter().any(|f| f == "explorer"),
            };
            let expires_at = ttl_secs.map(|s| {
                chrono::Utc::now()
                    .checked_add_signed(
                        chrono::TimeDelta::try_seconds(s).expect("ttl out of range"),
                    )
                    .expect("ttl out of range")
            });
            let key = signet_signer::generate_key();
            let challenge = key.challenge.clone();
            let secrets = EnvSecrets {
                signer_wif: key.wif,
                signer_pubkey: key.pubkey,
                signet_challenge: key.challenge,
                rpc_user: "signet".to_string(),
                rpc_password: uuid::Uuid::new_v4().simple().to_string(),
            };
            orchestrator
                .create_environment(
                    &env_id,
                    &secrets,
                    &signet_orchestrator::resolve_images(&None).unwrap(),
                    components,
                    None,
                    expires_at,
                )
                .await?;
            println!(
                "created env-{env_id} challenge={challenge} indexer={} faucet={} explorer={} ttl={:?}",
                components.indexer, components.faucet, components.explorer, ttl_secs
            );
        }
        "destroy" => {
            orchestrator.destroy_environment(&env_id).await?;
            println!("destroyed env-{env_id}");
        }
        "ready" => {
            println!("core ready: {}", orchestrator.core_ready(&env_id).await?);
        }
        other => panic!("unknown action {other}"),
    }
    Ok(())
}
