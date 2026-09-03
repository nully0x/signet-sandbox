use signet_orchestrator::{EnvSecrets, Orchestrator};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let action = args
        .next()
        .expect("usage: provision <create|destroy|ready> <env-id>");
    let env_id = args.next().expect("missing env id");

    let orchestrator = Orchestrator::connect().await?;
    match action.as_str() {
        "create" => {
            let key = signet_signer::generate_key();
            let challenge = key.challenge.clone();
            let secrets = EnvSecrets {
                signer_wif: key.wif,
                signer_pubkey: key.pubkey,
                signet_challenge: key.challenge,
                rpc_user: "signet".to_string(),
                rpc_password: uuid::Uuid::new_v4().simple().to_string(),
            };
            orchestrator.create_environment(&env_id, &secrets).await?;
            println!("created env-{env_id} challenge={challenge}");
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
