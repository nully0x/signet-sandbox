use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use signet_core::bundle::ConnectionBundle;
use signet_core::env::{BlockPolicy, EnvStatus};
use signet_db::{EnvironmentRow, PgPool};
use signet_nostr::{ApiToken, DEFAULT_MAX_AGE, verify_nip98_header};
use signet_orchestrator::{EnvSecrets, Orchestrator};
use signet_rpc::envelope::{Id, Request, Response};
use signet_rpc::error::{
    ENV_NOT_FOUND, Error, FORBIDDEN, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, PARSE_ERROR,
    UNAUTHENTICATED,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const NIP98_MAX_AGE: Duration = DEFAULT_MAX_AGE;

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    orchestrator: Arc<Orchestrator>,
    public_url: String,
}

pub struct Caller {
    #[allow(dead_code)]
    npub: String,
}

#[cfg(test)]
impl std::fmt::Debug for Caller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Caller").field("npub", &self.npub).finish()
    }
}

pub fn router(pool: PgPool, orchestrator: Orchestrator, public_url: String) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/rpc", post(dispatch))
        .with_state(AppState {
            pool,
            orchestrator: Arc::new(orchestrator),
            public_url,
        })
}

async fn healthz() -> &'static str {
    "ok"
}

async fn dispatch(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let request: Request = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(err) => {
            let code = if err.is_syntax() {
                PARSE_ERROR
            } else {
                INVALID_REQUEST
            };
            return (
                StatusCode::OK,
                Json(Response::error(None, Error::new(code, err.to_string()))),
            );
        }
    };

    let id = request.id.clone().unwrap_or(Id::Null);
    let caller = match authenticate(&state, &method, &headers).await {
        Ok(caller) => caller,
        Err(err) => return (StatusCode::OK, Json(Response::error(Some(id), err))),
    };

    let response = handle(&state, caller, request).await;
    (StatusCode::OK, Json(response))
}

async fn authenticate(
    state: &AppState,
    http_method: &Method,
    headers: &HeaderMap,
) -> Result<Caller, Error> {
    let header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| unauthenticated("missing Authorization header"))?;

    if let Some(payload) = header.strip_prefix("Nostr ") {
        return verify_nip98(payload, &state.public_url, http_method.as_str());
    }

    if let Some(raw) = header.strip_prefix("Bearer ") {
        return verify_bearer(&state.pool, raw).await;
    }

    Err(unauthenticated("unsupported Authorization scheme"))
}

fn verify_nip98(payload: &str, public_url: &str, method: &str) -> Result<Caller, Error> {
    let header = format!("Nostr {payload}");
    let url = format!("{public_url}/v1/rpc");
    match verify_nip98_header(&header, &url, method, NIP98_MAX_AGE) {
        Ok(npub) => Ok(Caller { npub }),
        Err(_) => Err(unauthenticated("NIP-98 authentication failed")),
    }
}

async fn verify_bearer(pool: &PgPool, raw: &str) -> Result<Caller, Error> {
    let token = ApiToken::parse(raw).map_err(|_| unauthenticated("malformed api token"))?;
    let row: Option<signet_db::ApiTokenRow> =
        signet_db::find_api_token(pool, token.hash().as_hex())
            .await
            .map_err(|_| Error::new(INTERNAL_ERROR, "token lookup failed"))?;

    match row {
        Some(row) if !row.revoked => Ok(Caller {
            npub: row.npub_owner,
        }),
        Some(_) => Err(unauthenticated("api token revoked")),
        None => Err(unauthenticated("unknown api token")),
    }
}

fn unauthenticated(message: &'static str) -> Error {
    Error::new(UNAUTHENTICATED, message)
}

async fn handle(state: &AppState, caller: Caller, request: Request) -> Response {
    let id = request.id.clone().unwrap_or(Id::Null);
    match request.method.as_str() {
        "environment.create" => environment_create(state, id, caller, request.params).await,
        "environment.get" => environment_get(state, id, request.params).await,
        "environment.destroy" => environment_destroy(state, id, caller, request.params).await,
        "environment.faucet" => Response::error(
            Some(id),
            Error::new(
                signet_rpc::error::NOT_AVAILABLE_IN_PHASE,
                "method environment.faucet not yet implemented",
            ),
        ),
        _ => Response::error(Some(id), Error::method_not_found(&request.method)),
    }
}

#[derive(Deserialize)]
struct CreateParams {
    name: String,
    #[serde(default)]
    block_policy: Option<BlockPolicy>,
    #[serde(default)]
    components: Components,
    #[serde(default)]
    versions: Option<BTreeMap<String, String>>,
    #[serde(default)]
    ttl_secs: Option<i64>,
}

#[derive(Deserialize, Default)]
struct Components {
    #[serde(default)]
    explorer: bool,
    #[serde(default)]
    indexer: bool,
    #[serde(default)]
    faucet: bool,
}

#[derive(Deserialize)]
struct IdParams {
    id: Uuid,
}

async fn environment_create(state: &AppState, id: Id, caller: Caller, params: Value) -> Response {
    let params: CreateParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };

    let images = match signet_orchestrator::resolve_images(&params.versions) {
        Ok(images) => images,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };
    let versions_json = serde_json::to_value(&images).ok();

    let env_id = Uuid::now_v7();
    let key = signet_signer::generate_key();
    let rpc_user = "signet".to_string();
    let rpc_password = Uuid::new_v4().simple().to_string();

    let secrets = EnvSecrets {
        signer_wif: key.wif,
        signer_pubkey: key.pubkey,
        signet_challenge: key.challenge.clone(),
        rpc_user: rpc_user.clone(),
        rpc_password: rpc_password.clone(),
    };

    let rpc_endpoint = format!("{}/env/{env_id}/rpc", state.public_url);
    let row = signet_db::NewEnvironment {
        id: env_id,
        name: &params.name,
        npub_owner: &caller.npub,
        block_policy: policy_as_str(params.block_policy.unwrap_or(BlockPolicy::Interval30s)),
        signet_challenge: &key.challenge,
        component_explorer: params.components.explorer,
        component_indexer: params.components.indexer,
        component_faucet: params.components.faucet,
        rpc_endpoint: &rpc_endpoint,
        ttl_secs: params.ttl_secs,
        expires_at: params
            .ttl_secs
            .map(|s| Utc::now() + chrono::Duration::seconds(s)),
        versions: versions_json,
    };

    let row = match signet_db::create_environment(&state.pool, &row).await {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(error = %e, "environment insert failed");
            return Response::error(
                Some(id),
                Error::new(INTERNAL_ERROR, "environment persist failed"),
            );
        }
    };

    if let Err(e) = state
        .orchestrator
        .create_environment(
            &short_id(env_id),
            &secrets,
            &images,
            params.components.faucet,
        )
        .await
    {
        tracing::error!(error = %e, "environment provisioning failed");
        let _ = signet_db::set_environment_status(&state.pool, env_id, "destroyed").await;
        return Response::error(
            Some(id),
            Error::new(INTERNAL_ERROR, "environment provisioning failed"),
        );
    }

    Response::result(
        Some(id),
        serde_json::to_value(bundle_for(
            &row,
            EnvStatus::Provisioning,
            Some(format!("{rpc_user}:{rpc_password}")),
        ))
        .expect("bundle serializes"),
    )
}

async fn environment_get(state: &AppState, id: Id, params: Value) -> Response {
    let params: IdParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };

    let row = match signet_db::get_environment(&state.pool, params.id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return Response::error(
                Some(id),
                Error::new(
                    ENV_NOT_FOUND,
                    format!("environment {} not found", params.id),
                ),
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "environment lookup failed");
            return Response::error(
                Some(id),
                Error::new(INTERNAL_ERROR, "environment lookup failed"),
            );
        }
    };

    let status = if row.status == "provisioning"
        && state
            .orchestrator
            .core_ready(&short_id(row.id))
            .await
            .unwrap_or(false)
    {
        let _ = signet_db::set_environment_status(&state.pool, row.id, "ready").await;
        EnvStatus::Ready
    } else {
        status_as_env(&row.status)
    };

    let rpc_auth = state
        .orchestrator
        .rpc_credentials(&short_id(row.id))
        .await
        .ok()
        .map(|(u, p)| format!("{u}:{p}"));

    Response::result(
        Some(id),
        serde_json::to_value(bundle_for(&row, status, rpc_auth)).expect("bundle serializes"),
    )
}

async fn environment_destroy(state: &AppState, id: Id, caller: Caller, params: Value) -> Response {
    let params: IdParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };

    let row = match signet_db::get_environment(&state.pool, params.id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return Response::error(
                Some(id),
                Error::new(
                    ENV_NOT_FOUND,
                    format!("environment {} not found", params.id),
                ),
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "environment lookup failed");
            return Response::error(
                Some(id),
                Error::new(INTERNAL_ERROR, "environment lookup failed"),
            );
        }
    };

    if row.npub_owner != caller.npub {
        return Response::error(
            Some(id),
            Error::new(FORBIDDEN, "caller is not the environment owner"),
        );
    }

    if let Err(e) = state
        .orchestrator
        .destroy_environment(&short_id(row.id))
        .await
    {
        tracing::error!(error = %e, "environment teardown failed");
        return Response::error(
            Some(id),
            Error::new(INTERNAL_ERROR, "environment teardown failed"),
        );
    }

    if let Err(e) = signet_db::set_environment_status(&state.pool, row.id, "destroyed").await {
        tracing::error!(error = %e, "environment status update failed");
    }

    Response::result(Some(id), serde_json::json!({ "destroyed": true }))
}

fn short_id(env_id: Uuid) -> String {
    let simple = env_id.simple().to_string();
    simple[..12].to_string()
}

fn policy_as_str(policy: BlockPolicy) -> &'static str {
    match policy {
        BlockPolicy::Interval30s => "interval_30s",
        BlockPolicy::OnDemand => "on_demand",
    }
}

fn status_as_env(status: &str) -> EnvStatus {
    match status {
        "ready" => EnvStatus::Ready,
        "expired" => EnvStatus::Expired,
        "destroyed" => EnvStatus::Destroyed,
        _ => EnvStatus::Provisioning,
    }
}

fn bundle_for(
    row: &EnvironmentRow,
    status: EnvStatus,
    rpc_auth: Option<String>,
) -> ConnectionBundle {
    ConnectionBundle {
        environment_id: row.id.to_string(),
        status,
        rpc_url: row.rpc_endpoint.clone(),
        rpc_auth,
        zmq_url: None,
        indexer_url: row.indexer_endpoint.clone(),
        explorer_url: row.explorer_endpoint.clone(),
        faucet_url: row.faucet_endpoint.clone(),
        lightning: None,
        signet_challenge: row.signet_challenge.clone(),
        block_policy: match row.block_policy.as_str() {
            "on_demand" => BlockPolicy::OnDemand,
            _ => BlockPolicy::Interval30s,
        },
        versions: row
            .versions
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
        expires_at: row.expires_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use nostr::event::{Event, EventBuilder, FinalizeEvent, Kind, Tag};
    use nostr::key::Keys;
    use nostr::nips::nip19::ToBech32 as _;
    use nostr::types::Timestamp;

    const PUBLIC_URL: &str = "http://localhost:8080";
    const RPC_URL: &str = "http://localhost:8080/v1/rpc";

    fn auth_header(keys: &Keys, url: &str, method: &str) -> String {
        let event = EventBuilder::new(Kind::HttpAuth, "")
            .tags(vec![
                Tag::custom("u", [url]),
                Tag::custom("method", [method]),
            ])
            .custom_created_at(Timestamp::now())
            .finalize(keys)
            .unwrap();
        format!(
            "Nostr {}",
            BASE64.encode(serde_json::to_vec(&event).unwrap())
        )
    }

    fn verified(header: &str) -> Result<Caller, Error> {
        verify_nip98(header.strip_prefix("Nostr ").unwrap(), PUBLIC_URL, "POST")
    }

    fn is_unauthenticated(err: &Error) -> bool {
        err.code == UNAUTHENTICATED
    }

    #[test]
    fn valid_nip98_header_authenticates() {
        let keys = Keys::parse(&format!("{:064x}", 7)).unwrap();
        let caller = verified(&auth_header(&keys, RPC_URL, "POST")).unwrap();
        assert_eq!(caller.npub, keys.public_key().to_bech32().unwrap());
    }

    #[test]
    fn nip98_url_mismatch_is_unauthenticated() {
        let keys = Keys::parse(&format!("{:064x}", 7)).unwrap();
        let header = auth_header(&keys, "http://evil.example/v1/rpc", "POST");
        assert!(is_unauthenticated(&verified(&header).unwrap_err()));
    }

    #[test]
    fn nip98_method_mismatch_is_unauthenticated() {
        let keys = Keys::parse(&format!("{:064x}", 7)).unwrap();
        let header = auth_header(&keys, RPC_URL, "GET");
        assert!(is_unauthenticated(&verified(&header).unwrap_err()));
    }

    #[test]
    fn tampered_nip98_payload_is_unauthenticated() {
        let keys = Keys::parse(&format!("{:064x}", 7)).unwrap();
        let event = EventBuilder::new(Kind::HttpAuth, "")
            .tags(vec![
                Tag::custom("u", [RPC_URL]),
                Tag::custom("method", ["POST"]),
            ])
            .custom_created_at(Timestamp::now())
            .finalize(&keys)
            .unwrap();
        let tampered = Event::new(
            event.id,
            event.pubkey,
            event.created_at,
            event.kind,
            event.tags.to_vec(),
            "tampered",
            event.sig,
        );
        let header = format!(
            "Nostr {}",
            BASE64.encode(serde_json::to_vec(&tampered).unwrap())
        );
        assert!(is_unauthenticated(&verified(&header).unwrap_err()));
    }

    #[test]
    fn stale_nip98_event_is_unauthenticated() {
        let keys = Keys::parse(&format!("{:064x}", 7)).unwrap();
        let event = EventBuilder::new(Kind::HttpAuth, "")
            .tags(vec![
                Tag::custom("u", [RPC_URL]),
                Tag::custom("method", ["POST"]),
            ])
            .custom_created_at(Timestamp::now() - Duration::from_secs(3600))
            .finalize(&keys)
            .unwrap();
        let header = format!(
            "Nostr {}",
            BASE64.encode(serde_json::to_vec(&event).unwrap())
        );
        assert!(is_unauthenticated(&verified(&header).unwrap_err()));
    }

    #[test]
    fn create_params_default_components_and_policy() {
        let params: CreateParams =
            serde_json::from_value(serde_json::json!({ "name": "x" })).unwrap();
        assert_eq!(params.block_policy, None);
        assert!(!params.components.explorer);
        assert_eq!(params.ttl_secs, None);
    }

    #[test]
    fn bundle_omits_disabled_components() {
        let row = EnvironmentRow {
            id: Uuid::now_v7(),
            name: "x".into(),
            npub_owner: "npub1x".into(),
            workspace_id: None,
            status: "provisioning".into(),
            block_policy: "interval_30s".into(),
            signet_challenge: "512100ae".into(),
            component_explorer: false,
            component_indexer: false,
            component_faucet: false,
            component_lightning: None,
            rpc_endpoint: "http://x/rpc".into(),
            indexer_endpoint: None,
            explorer_endpoint: None,
            faucet_endpoint: None,
            ln_endpoint: None,
            ttl_secs: None,
            created_at: Utc::now(),
            expires_at: None,
            current_snapshot_id: None,
            versions: None,
        };
        let json = serde_json::to_value(bundle_for(&row, EnvStatus::Provisioning, None)).unwrap();
        for absent in [
            "indexer_url",
            "explorer_url",
            "faucet_url",
            "lightning",
            "versions",
        ] {
            assert!(!json.as_object().unwrap().contains_key(absent));
        }
    }

    #[test]
    fn bundle_echoes_resolved_versions() {
        let mut row = EnvironmentRow {
            id: Uuid::now_v7(),
            name: "x".into(),
            npub_owner: "npub1x".into(),
            workspace_id: None,
            status: "ready".into(),
            block_policy: "interval_30s".into(),
            signet_challenge: "512100ae".into(),
            component_explorer: false,
            component_indexer: false,
            component_faucet: false,
            component_lightning: None,
            rpc_endpoint: "http://x/rpc".into(),
            indexer_endpoint: None,
            explorer_endpoint: None,
            faucet_endpoint: None,
            ln_endpoint: None,
            ttl_secs: None,
            created_at: Utc::now(),
            expires_at: None,
            current_snapshot_id: None,
            versions: None,
        };
        row.versions = Some(serde_json::json!({ "bitcoind": "bitcoin/bitcoin:28.1" }));
        let json = serde_json::to_value(bundle_for(&row, EnvStatus::Ready, None)).unwrap();
        assert_eq!(
            json["versions"]["bitcoind"],
            serde_json::json!("bitcoin/bitcoin:28.1")
        );
    }
}
