// `signet` — CLI for the Signet Sandbox provisioning API.

mod auth;
mod config;
mod duration;
mod output;
mod rpc;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use serde_json::json;
use signet_core::{ConnectionBundle, EnvStatus};

use crate::rpc::{CallError, Client};

#[derive(Debug)]
enum CliError {
    Rpc(signet_rpc::error::Error),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for CliError {
    fn from(err: anyhow::Error) -> Self {
        CliError::Other(err)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(err: serde_json::Error) -> Self {
        CliError::Other(err.into())
    }
}

impl From<CallError> for CliError {
    fn from(err: CallError) -> Self {
        match err {
            CallError::Rpc(e) => CliError::Rpc(e),
            other => CliError::Other(anyhow::anyhow!("{other}")),
        }
    }
}

#[derive(Parser)]
#[command(
    name = "signet",
    version,
    about = "Provision and drive Signet Sandbox environments"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Provision an environment and wait until it is ready
    Up {
        /// Path to the environment config file (JSON, spec §5)
        #[arg(long)]
        config: Option<PathBuf>,
        /// Environment name (overrides the config file)
        #[arg(long)]
        name: Option<String>,
        /// Time to live, e.g. 20m, 1h, 2d (overrides the config file)
        #[arg(long)]
        ttl: Option<String>,
        /// Give up waiting for readiness after this many seconds
        #[arg(long, default_value_t = 300)]
        timeout_secs: u64,
        /// Seconds between readiness polls
        #[arg(long, default_value_t = 2)]
        poll_secs: u64,
        #[command(flatten)]
        common: Common,
    },
    /// Fetch an environment's connection bundle
    Get {
        /// Environment id (UUID)
        env_id: String,
        #[command(flatten)]
        common: Common,
    },
    /// Fund an address from the environment's faucet
    Fund {
        /// Environment id (UUID)
        env_id: String,
        /// Destination signet address
        address: String,
        /// Amount to mint, in sats
        #[arg(long)]
        amount: u64,
        #[command(flatten)]
        common: Common,
    },
    /// Destroy an environment
    Down {
        /// Environment id (UUID)
        env_id: String,
        #[command(flatten)]
        common: Common,
    },
}

#[derive(Args)]
struct Common {
    /// API base URL
    #[arg(long, env = "SIGNET_API", default_value = "http://localhost:8081")]
    api: String,
    /// API bearer token (sgn_…)
    #[arg(long, env = "SIGNET_TOKEN")]
    token: Option<String>,
    /// Nostr secret key (nsec… or hex) for NIP-98 auth
    #[arg(long, env = "SIGNET_NSEC")]
    nsec: Option<String>,
    /// Print raw JSON results
    #[arg(long)]
    json: bool,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    std::process::ExitCode::from(run(cli.command))
}

fn run(command: Command) -> u8 {
    let result = match command {
        Command::Up {
            config,
            name,
            ttl,
            timeout_secs,
            poll_secs,
            common,
        } => up(config, name, ttl, timeout_secs, poll_secs, &common),
        Command::Get { env_id, common } => get(&env_id, &common),
        Command::Fund {
            env_id,
            address,
            amount,
            common,
        } => fund(&env_id, &address, amount, &common),
        Command::Down { env_id, common } => down(&env_id, &common),
    };
    match result {
        Ok(()) => 0,
        Err(CliError::Rpc(e)) => {
            eprintln!("rpc error {}: {}", e.code, e.message);
            1
        }
        Err(CliError::Other(e)) => {
            eprintln!("error: {:#}", e);
            1
        }
    }
}

fn client(common: &Common) -> anyhow::Result<Client> {
    let auth = auth::from_flags(common.token.clone(), common.nsec.clone())?;
    Client::new(&common.api, auth)
}

fn up(
    config_path: Option<PathBuf>,
    name: Option<String>,
    ttl: Option<String>,
    timeout_secs: u64,
    poll_secs: u64,
    common: &Common,
) -> Result<(), CliError> {
    let client = client(common)?;
    let cfg = match &config_path {
        Some(path) => Some(config::load_config(path)?),
        None => None,
    };
    let params = config::build_create_params(cfg.as_ref(), name, ttl)?;

    let started = Instant::now();
    let mut bundle: ConnectionBundle =
        serde_json::from_value(client.call("environment.create", serde_json::to_value(params)?)?)
            .map_err(unexpected_payload)?;

    if bundle.status != EnvStatus::Ready {
        eprintln!("environment {} provisioning…", bundle.environment_id);
        let deadline = started + Duration::from_secs(timeout_secs);
        loop {
            if Instant::now() >= deadline {
                return Err(CliError::Other(anyhow::anyhow!(
                    "timed out waiting for readiness after {timeout_secs}s"
                )));
            }
            std::thread::sleep(Duration::from_secs(poll_secs));
            bundle = poll_get(&client, &bundle.environment_id)?;
            match bundle.status {
                EnvStatus::Ready => break,
                EnvStatus::Provisioning => continue,
                other => {
                    return Err(CliError::Other(anyhow::anyhow!(
                        "environment reached terminal status '{}' while provisioning",
                        output::status_label(other)
                    )));
                }
            }
        }
        eprintln!("ready after {}s", started.elapsed().as_secs());
    }

    if common.json {
        output::print_json(&bundle)?;
    } else {
        output::print_bundle_human(&bundle);
    }
    Ok(())
}

fn get(env_id: &str, common: &Common) -> Result<(), CliError> {
    let client = client(common)?;
    let bundle = poll_get(&client, env_id)?;
    if common.json {
        output::print_json(&bundle)?;
    } else {
        output::print_bundle_human(&bundle);
    }
    Ok(())
}

fn fund(env_id: &str, address: &str, amount: u64, common: &Common) -> Result<(), CliError> {
    let client = client(common)?;
    let result = client.call(
        "environment.faucet",
        json!({ "id": env_id, "address": address, "amount_sat": amount }),
    )?;
    if common.json {
        output::print_json(&result)?;
    } else {
        let txid = result
            .get("txid")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<no txid returned>");
        println!("txid: {txid}");
    }
    Ok(())
}

fn down(env_id: &str, common: &Common) -> Result<(), CliError> {
    let client = client(common)?;
    let result = client.call("environment.destroy", json!({ "id": env_id }))?;
    if common.json {
        output::print_json(&result)?;
    } else {
        let destroyed = result
            .get("destroyed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !destroyed {
            return Err(CliError::Other(anyhow::anyhow!(
                "environment.destroy did not confirm destruction"
            )));
        }
        println!("environment {env_id} destroyed");
    }
    Ok(())
}

fn poll_get(client: &Client, env_id: &str) -> anyhow::Result<ConnectionBundle> {
    serde_json::from_value(client.call("environment.get", json!({ "id": env_id }))?)
        .map_err(unexpected_payload)
}

fn unexpected_payload(err: serde_json::Error) -> anyhow::Error {
    anyhow::anyhow!("API returned an unexpected payload: {err}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn rpc_result(payload: &str) -> String {
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{payload}}}")
    }

    fn bundle_json(status: &str) -> String {
        format!(
            "{{\"environment_id\":\"0195c7c5-0000-7000-8000-000000000000\",\"status\":\"{status}\",\"rpc_url\":\"http://x/env/rpc\",\"rpc_auth\":\"signet:pw\",\"signet_challenge\":\"5121\",\"block_policy\":\"interval_30s\"}}"
        )
    }

    /// Serve the given bodies in sequence, one per connection.
    fn serve_sequence(bodies: Vec<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for body in bodies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf).unwrap();
                let http = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(http.as_bytes()).unwrap();
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn up_polls_until_ready_then_prints_the_bundle() {
        let base = serve_sequence(vec![
            rpc_result(&bundle_json("provisioning")),
            rpc_result(&bundle_json("provisioning")),
            rpc_result(&bundle_json("ready")),
        ]);
        let common = Common {
            api: base,
            token: Some("sgn_t".into()),
            nsec: None,
            json: false,
        };
        let result = up(None, Some("cli-test".into()), None, 10, 0, &common);
        assert!(result.is_ok(), "up failed");
    }

    #[test]
    fn up_fails_on_a_terminal_status_during_provisioning() {
        let base = serve_sequence(vec![
            rpc_result(&bundle_json("provisioning")),
            rpc_result(&bundle_json("destroyed")),
        ]);
        let common = Common {
            api: base,
            token: Some("sgn_t".into()),
            nsec: None,
            json: false,
        };
        assert!(up(None, Some("cli-test".into()), None, 10, 0, &common).is_err());
    }
}
