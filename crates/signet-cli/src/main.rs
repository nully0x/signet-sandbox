// `signet` — CLI for the Signet Sandbox provisioning API.

mod auth;
mod config;
mod duration;
mod local;
mod output;
mod profile;
mod rpc;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use nostr::key::Keys;
use serde_json::json;
use signet_core::{ConnectionBundle, EnvStatus};

use crate::auth::Auth;
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
            CallError::Transport(e) if e.is_connect() => {
                let url = e.url().map(reqwest::Url::as_str).unwrap_or("the API");
                CliError::Other(anyhow::anyhow!(
                    "cannot reach the API at {url} — is it running? start it with: just dev-api"
                ))
            }
            other => CliError::Other(anyhow::Error::new(other)),
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
        /// Path to the environment config file (JSON)
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
        /// Open the dashboard in a browser once the environment is ready
        #[arg(long)]
        ui: bool,
        /// Host advertised in endpoint URLs when the stack gets
        /// bootstrapped (IP or domain); defaults to localhost, which
        /// limits endpoints to this machine
        #[arg(long, default_value = "localhost")]
        host: String,
        #[command(flatten)]
        common: Common,
    },
    /// Fetch an environment's connection bundle
    Get {
        /// Environment id or name
        env: String,
        #[command(flatten)]
        common: Common,
    },
    /// List your environments
    Ls {
        #[command(flatten)]
        common: Common,
    },
    /// Suspend an environment's compute (storage persists)
    Stop {
        /// Environment id or name
        env: String,
        #[command(flatten)]
        common: Common,
    },
    /// Resume a stopped environment
    Start {
        /// Environment id or name
        env: String,
        #[command(flatten)]
        common: Common,
    },
    /// Fund an address from the environment's faucet
    Fund {
        /// Environment id or name
        env: String,
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
        /// Environment id or name
        env: String,
        #[command(flatten)]
        common: Common,
    },
    /// Remove a destroyed or expired environment's record
    Delete {
        /// Environment id or name
        env: String,
        #[command(flatten)]
        common: Common,
    },
    /// Mint a bearer API token (NIP-98-only; prints the raw token)
    ///
    /// Without --nsec, signs with the local profile identity from
    /// ~/.signet/config.toml (created by `signet local init`).
    #[command(
        after_help = "Examples:\n  signet token                  # mint with the local profile identity\n  signet token --nsec <key>     # mint with an explicit key\n  signet token | sed 's/^/bearer=/'"
    )]
    Token {
        /// Nostr secret key (nsec… or hex) that signs the mint request
        #[arg(long, env = "SIGNET_NSEC")]
        nsec: Option<String>,
        /// API base URL
        #[arg(long, env = "SIGNET_API", default_value = "http://localhost:8081")]
        api: String,
        /// Print raw JSON results
        #[arg(long)]
        json: bool,
    },
    /// Print the JSON Schema for the environment config
    Schema {},
    /// Create or start the local cluster and its platform stack
    Bootstrap {
        /// Host advertised in endpoint URLs (IP or domain); defaults to
        /// localhost, which limits endpoints to this machine
        #[arg(long, default_value = "localhost")]
        host: String,
    },
    /// Report the local platform, api, and identity state
    Status,
    /// Delete the local cluster (environments go with it; the profile is kept)
    Teardown,
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
            ui,
            host,
            common,
        } => up(
            &UpOptions {
                config,
                name,
                ttl,
                timeout_secs,
                poll_secs,
                ui,
                host,
            },
            &common,
        ),
        Command::Get { env, common } => get(&env, &common),
        Command::Ls { common } => ls(&common),
        Command::Stop { env, common } => lifecycle("environment.stop", &env, &common),
        Command::Start { env, common } => lifecycle("environment.start", &env, &common),
        Command::Fund {
            env,
            address,
            amount,
            common,
        } => fund(&env, &address, amount, &common),
        Command::Down { env, common } => down(&env, &common),
        Command::Delete { env, common } => delete(&env, &common),
        Command::Token { nsec, api, json } => token(nsec, &api, json),
        Command::Schema {} => schema(),
        Command::Bootstrap { host } => local::bootstrap(&host).map_err(CliError::from),
        Command::Status => local::status().map_err(CliError::from),
        Command::Teardown => local::teardown().map_err(CliError::from),
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
    let auth = match auth::from_flags(common.token.clone(), common.nsec.clone()) {
        Ok(auth) => auth,
        // No explicit credentials: fall back to the local profile token.
        Err(_) if local::profile_credentials_apply(common) => {
            let profile = crate::profile::load()?;
            auth::Auth::Bearer(profile.token)
        }
        Err(e) => return Err(e),
    };
    Client::new(&common.api, auth)
}

/// `up` is the auto-bootstrap entry point: when the default local
/// api is not running, bring the whole local stack up first. A custom
/// `--api` that is down stays a hard error.
struct UpOptions {
    config: Option<PathBuf>,
    name: Option<String>,
    ttl: Option<String>,
    timeout_secs: u64,
    poll_secs: u64,
    ui: bool,
    host: String,
}

fn ensure_local_api(common: &Common, host: &str) -> Result<(), CliError> {
    let reachable = local::api_healthy(&common.api);
    if local::should_bootstrap(&common.api, reachable) {
        println!(
            "no local api on {} — bootstrapping the local cluster",
            local::DEFAULT_LOCAL_API
        );
        return local::bootstrap(host).map_err(CliError::from);
    }
    if !reachable {
        return Err(CliError::Other(anyhow::anyhow!(
            "cannot reach the API at {} — start it with: signet bootstrap (or signet up)",
            common.api
        )));
    }
    Ok(())
}

fn up(opts: &UpOptions, common: &Common) -> Result<(), CliError> {
    let UpOptions {
        config,
        name,
        ttl,
        timeout_secs,
        poll_secs,
        ui,
        host,
    } = opts;
    let timeout_secs = *timeout_secs;
    let poll_secs = *poll_secs;
    let ui = *ui;
    let host = host.as_str();
    let config_path = config.clone();
    let name = name.clone();
    let ttl = ttl.clone();
    ensure_local_api(common, host)?;
    let client = match heal_stale_token(common)? {
        Some(client) => client,
        None => client(common)?,
    };
    // Project convention: a sandbox.json in the working directory is the
    // default config when no --config or --name is given.
    let cfg = match &config_path {
        Some(path) => Some(config::load_config(path)?),
        None => {
            let default_path = std::path::Path::new("sandbox.json");
            if default_path.exists() {
                Some(config::load_config(default_path)?)
            } else {
                None
            }
        }
    };
    let params = config::build_create_params(cfg.as_ref(), name, ttl)?;
    // The create call consumes params; keep the instance names for the
    // wallet wait below.
    let lightning_names: Vec<String> = params
        .components
        .lightning
        .iter()
        .map(|l| l.name.clone())
        .collect();

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

    // Lightning wallets self-initialize after the pods start; a bundle
    // carries an instance only once its wallet answers. Callers that
    // asked for lightning almost always want the nodes, so wait here —
    // CI tolerates the extra minute.
    if !lightning_names.is_empty() {
        eprintln!("waiting for lightning wallets…");
        let deadline = started + Duration::from_secs(timeout_secs);
        loop {
            bundle = poll_get(&client, &bundle.environment_id)?;
            let have: Vec<_> = lightning_names
                .iter()
                .filter(|l| !bundle.lightning.contains_key(*l))
                .cloned()
                .collect();
            if have.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(CliError::Other(anyhow::anyhow!(
                    "timed out waiting for lightning wallets after {timeout_secs}s"
                )));
            }
            std::thread::sleep(Duration::from_secs(poll_secs));
        }
        eprintln!(
            "lightning wallets up after {}s",
            started.elapsed().as_secs()
        );
    }

    if common.json {
        output::print_json(&bundle)?;
    } else {
        output::print_bundle_human(&bundle);
    }
    if ui {
        open_dashboard(common)?;
    }
    Ok(())
}

/// A stored bearer token does not survive database recreates: the
/// profile's `token` field may predate the api currently running. When
/// the credentials are bearer-based (explicit or profile fallback) and
/// an nsec is available, probe once and heal a rejected token by
/// re-minting against the live api. Explicit `--nsec` callers sign per
/// request and cannot go stale.
fn heal_stale_token(common: &Common) -> Result<Option<Client>, CliError> {
    if common.token.is_some() || common.nsec.is_some() {
        return Ok(None);
    }
    let profile = match crate::profile::load() {
        Ok(profile) => profile,
        Err(_) => return Ok(None),
    };
    if profile.nsec.is_empty() {
        return Ok(None);
    }
    let probe = Client::new(&common.api, auth::Auth::Bearer(profile.token.clone()))?;
    match probe.call("environment.list", json!({})) {
        Ok(_) => Ok(None),
        Err(CallError::Rpc(e)) if e.code == signet_rpc::error::UNAUTHENTICATED => {
            eprintln!("profile token rejected — minting a fresh one");
            let token = mint_token(&common.api, &profile.nsec)?;
            let _ = crate::profile::refresh_token(&token);
            Ok(Some(Client::new(&common.api, auth::Auth::Bearer(token))?))
        }
        Err(_) => Ok(None),
    }
}

/// Hand the browser a token valid for the api being talked to: an
/// explicit `--token` is reused as-is, otherwise the profile token (just
/// healed by the pre-flight probe, or minted on the spot) is used.
fn open_dashboard(common: &Common) -> Result<(), CliError> {
    let token = match common.token.clone() {
        Some(token) => token,
        None => match crate::profile::load() {
            Ok(profile) if !profile.token.is_empty() => profile.token,
            _ => {
                let key = common
                    .nsec
                    .clone()
                    .or_else(|| crate::profile::load().ok().map(|p| p.nsec))
                    .ok_or_else(|| anyhow::anyhow!("no credentials to hand to the dashboard"))?;
                mint_token(&common.api, &key)?
            }
        },
    };
    let url = format!("{}/#token={token}", common.api);
    if open_browser(&url) {
        eprintln!("dashboard opened");
    } else {
        eprintln!("open the dashboard: {url}");
    }
    Ok(())
}

fn open_browser(url: &str) -> bool {
    use std::process::{Command, Stdio};
    let spawn = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    };
    if cfg!(target_os = "macos") {
        spawn("open", &[url])
    } else if cfg!(target_os = "windows") {
        spawn("cmd", &["/C", "start", "", url])
    } else {
        spawn("xdg-open", &[url])
    }
}

fn get(env: &str, common: &Common) -> Result<(), CliError> {
    let client = client(common)?;
    let bundle = poll_get(&client, env)?;
    if common.json {
        output::print_json(&bundle)?;
    } else {
        output::print_bundle_human(&bundle);
    }
    Ok(())
}

fn ls(common: &Common) -> Result<(), CliError> {
    let client = client(common)?;
    let result = client.call("environment.list", json!({}))?;
    if common.json {
        output::print_json(&result)?;
        return Ok(());
    }
    let rows: Vec<output::ListRow> = serde_json::from_value(result).map_err(unexpected_payload)?;
    print!("{}", output::render_list(&rows));
    Ok(())
}

fn lifecycle(method: &str, env: &str, common: &Common) -> Result<(), CliError> {
    let client = client(common)?;
    let result = client.call(method, json!({ "id": env }))?;
    if common.json {
        output::print_json(&result)?;
        return Ok(());
    }
    let status = result
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    println!("environment {env}: {status}");
    if status == "provisioning" {
        println!("poll with: signet get {env}");
    }
    Ok(())
}

fn schema() -> Result<(), CliError> {
    let schema = schemars::schema_for!(signet_core::CreateParams);
    output::print_json(&schema)?;
    Ok(())
}

fn fund(env: &str, address: &str, amount: u64, common: &Common) -> Result<(), CliError> {
    let client = client(common)?;
    let result = client.call(
        "environment.faucet",
        json!({ "id": env, "address": address, "amount_sat": amount }),
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

fn down(env: &str, common: &Common) -> Result<(), CliError> {
    let client = client(common)?;
    let result = client.call("environment.destroy", json!({ "id": env }))?;
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
        println!("environment {env} destroyed");
    }
    Ok(())
}

fn delete(env: &str, common: &Common) -> Result<(), CliError> {
    let client = client(common)?;
    let result = client.call("environment.delete", json!({ "id": env }))?;
    if common.json {
        output::print_json(&result)?;
    } else {
        let deleted = result
            .get("deleted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !deleted {
            return Err(CliError::Other(anyhow::anyhow!(
                "environment.delete did not confirm deletion"
            )));
        }
        println!("environment {env} deleted");
    }
    Ok(())
}

pub(crate) fn mint_token(api: &str, key: &str) -> anyhow::Result<String> {
    let keys = Keys::parse(key).map_err(|e| anyhow::anyhow!("invalid Nostr secret key: {e}"))?;
    let client = Client::new(api, Auth::Nip98(keys))?;
    let result = client.call("token.create", json!({}))?;
    result
        .get("token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("token.create returned no token"))
}

fn token(nsec: Option<String>, api: &str, json: bool) -> Result<(), CliError> {
    // No explicit key: mint from the local profile identity (same fallback
    // rule as every other command's credential resolution).
    let key = match nsec.as_deref().filter(|k| !k.is_empty()) {
        Some(key) => key.to_string(),
        None => {
            let profile = crate::profile::load().map_err(|e| {
                anyhow::anyhow!(
                    "no NIP-98 key and no profile — pass --nsec, set SIGNET_NSEC, or run signet local init ({e})"
                )
            })?;
            profile.nsec
        }
    };
    let raw = mint_token(api, &key)?;
    if json {
        output::print_json(&json!({ "token": raw }))?;
    } else {
        println!("{raw}");
    }
    Ok(())
}

fn poll_get(client: &Client, env: &str) -> anyhow::Result<ConnectionBundle> {
    serde_json::from_value(client.call("environment.get", json!({ "id": env }))?)
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
        // First request is the up() health probe; it only checks the status.
        let base = serve_sequence(vec![
            "ok".to_string(),
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
        let result = up(
            &UpOptions {
                config: None,
                name: Some("cli-test".into()),
                ttl: None,
                timeout_secs: 10,
                poll_secs: 0,
                ui: false,
                host: "localhost".into(),
            },
            &common,
        );
        if let Err(e) = &result {
            eprintln!("up error: {e:?}");
        }
        assert!(result.is_ok(), "up failed");
    }

    #[test]
    fn up_fails_on_a_terminal_status_during_provisioning() {
        let base = serve_sequence(vec![
            "ok".to_string(),
            rpc_result(&bundle_json("provisioning")),
            rpc_result(&bundle_json("destroyed")),
        ]);
        let common = Common {
            api: base,
            token: Some("sgn_t".into()),
            nsec: None,
            json: false,
        };
        assert!(
            up(
                &UpOptions {
                    config: None,
                    name: Some("cli-test".into()),
                    ttl: None,
                    timeout_secs: 10,
                    poll_secs: 0,
                    ui: false,
                    host: "localhost".into(),
                },
                &common,
            )
            .is_err()
        );
    }

    #[test]
    fn token_mints_with_nip98_and_prints_the_raw_token() {
        let base = serve_sequence(vec![rpc_result(r#"{"token":"sgn_00ab"}"#)]);
        let key = "0000000000000000000000000000000000000000000000000000000000000001";
        assert!(
            token(Some(key.into()), &base, false).is_ok(),
            "token failed"
        );
    }

    #[test]
    fn token_requires_a_nip98_key() {
        let base = serve_sequence(vec![]);
        assert!(token(None, &base, false).is_err());
    }

    #[test]
    fn token_rejects_an_invalid_key() {
        let base = serve_sequence(vec![]);
        assert!(token(Some("zzz".into()), &base, false).is_err());
    }

    #[test]
    fn connect_failures_hint_at_the_remedy() {
        let client = Client::new("http://127.0.0.1:1", Auth::Bearer("sgn_t".into())).unwrap();
        let err = CliError::from(client.call("environment.get", json!({})).unwrap_err());
        match err {
            CliError::Other(msg) => {
                let shown = format!("{msg:#}");
                assert!(shown.contains("cannot reach the API"), "got: {shown}");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn unreachable_local_api_points_at_bootstrap() {
        // Probe the decision the same way ensure_local_api does.
        assert!(local::should_bootstrap(local::DEFAULT_LOCAL_API, false));
        assert!(!local::should_bootstrap("http://hosted.example", false));
    }
}
