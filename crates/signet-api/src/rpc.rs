use axum::extract::{OriginalUri, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use signet_core::bundle::{ConnectionBundle, LightningConnection};
use signet_core::env::{BlockPolicy, CreateParams, EnvStatus};
use signet_core::versions::VersionCatalog;
use signet_db::{EnvironmentRow, PgPool};
use signet_nostr::{ApiToken, verify_nip98_header};
use signet_orchestrator::{EnvComponents, EnvSecrets, Orchestrator};
use signet_rpc::envelope::{Id, Request, Response};
use signet_rpc::error::{
    CHAIN_ERROR, ENV_NOT_FOUND, Error, FAUCET_FAILED, FORBIDDEN, INTERNAL_ERROR, INVALID_ADDRESS,
    INVALID_PARAMS, INVALID_REQUEST, NOT_AVAILABLE_IN_PHASE, PARSE_ERROR, UNAUTHENTICATED,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    orchestrator: Arc<Orchestrator>,
    catalog: VersionCatalog,
    public_url: String,
    nip98_max_age: Option<Duration>,
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

pub fn router(
    pool: PgPool,
    orchestrator: Orchestrator,
    catalog: VersionCatalog,
    public_url: String,
    nip98_max_age: Option<Duration>,
) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/rpc", post(dispatch))
        .fallback(crate::dashboard::dashboard)
        .with_state(AppState {
            catalog,
            pool,
            orchestrator: Arc::new(orchestrator),
            public_url,
            nip98_max_age,
        })
}

async fn healthz() -> &'static str {
    "ok"
}

async fn dispatch(
    State(state): State<AppState>,
    uri: OriginalUri,
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
    let url = request_url(&headers, uri.path());
    let (caller, via_nip98) = match authenticate(&state, &url, &method, &headers).await {
        Ok(auth) => auth,
        Err(err) => return (StatusCode::OK, Json(Response::error(Some(id), err))),
    };

    let response = handle(&state, caller, via_nip98, request).await;
    (StatusCode::OK, Json(response))
}

/// The URL the request actually hit, per NIP-98: scheme, authority,
/// path. The scheme comes from `X-Forwarded-Proto` (first hop wins when
/// several proxies chained) so TLS-terminating edges verify https URLs;
/// direct connections are plain http. The authority is the `Host`
/// header, which TCP passthroughs and proxies preserve. This replaces
/// the pinned public URL: clients sign whatever URL they use, so the
/// same API verifies localhost and LAN callers.
fn request_url(headers: &HeaderMap, path: &str) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .unwrap_or("http");
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    format!("{scheme}://{host}{path}")
}

async fn authenticate(
    state: &AppState,
    url: &str,
    http_method: &Method,
    headers: &HeaderMap,
) -> Result<(Caller, bool), Error> {
    let header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| unauthenticated("missing Authorization header"))?;

    if let Some(payload) = header.strip_prefix("Nostr ") {
        return verify_nip98(payload, url, http_method.as_str(), state.nip98_max_age)
            .map(|c| (c, true));
    }

    if let Some(raw) = header.strip_prefix("Bearer ") {
        return verify_bearer(&state.pool, raw).await.map(|c| (c, false));
    }

    Err(unauthenticated("unsupported Authorization scheme"))
}

fn verify_nip98(
    payload: &str,
    url: &str,
    method: &str,
    max_age: Option<Duration>,
) -> Result<Caller, Error> {
    let header = format!("Nostr {payload}");
    match verify_nip98_header(&header, url, method, max_age) {
        Ok(npub) => Ok(Caller { npub }),
        Err(e) => Err(Error::new(
            UNAUTHENTICATED,
            format!("NIP-98 authentication failed: {e}"),
        )),
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

async fn handle(state: &AppState, caller: Caller, via_nip98: bool, request: Request) -> Response {
    let id = request.id.clone().unwrap_or(Id::Null);
    match request.method.as_str() {
        // Bearer callers must not mint new tokens; issuance is NIP-98-only.
        "token.create" if !via_nip98 => Response::error(
            Some(id),
            unauthenticated("token.create requires NIP-98 authentication"),
        ),
        "token.create" => token_create(state, id, caller).await,
        "catalog.list" => catalog_list(state, id).await,
        "environment.create" => environment_create(state, id, caller, request.params).await,
        "environment.list" => environment_list(state, id, caller).await,
        "environment.get" => environment_get(state, id, caller, request.params).await,
        "environment.update" => environment_update(state, id, caller, request.params).await,
        "environment.stop" => environment_stop(state, id, caller, request.params).await,
        "environment.start" => environment_start(state, id, caller, request.params).await,
        "environment.destroy" => environment_destroy(state, id, caller, request.params).await,
        "environment.delete" => environment_delete(state, id, caller, request.params).await,
        "environment.faucet" => environment_faucet(state, id, caller, request.params).await,
        _ => Response::error(Some(id), Error::method_not_found(&request.method)),
    }
}

/// The version catalog for UI dropdowns and client-side validation.
async fn catalog_list(state: &AppState, id: Id) -> Response {
    Response::result(
        Some(id),
        serde_json::to_value(state.catalog.entries()).expect("catalog serializes"),
    )
}

async fn token_create(state: &AppState, id: Id, caller: Caller) -> Response {
    let token = ApiToken::generate();
    match signet_db::insert_api_token(&state.pool, token.hash().as_hex(), &caller.npub).await {
        Ok(()) => Response::result(Some(id), serde_json::json!({ "token": token.as_str() })),
        Err(e) => {
            tracing::error!(error = %e, "api token insert failed");
            Response::error(Some(id), Error::new(INTERNAL_ERROR, "token persist failed"))
        }
    }
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

enum EnvRef {
    Id(Uuid),
    Name(String),
}

fn parse_env_ref(raw: &str) -> EnvRef {
    match Uuid::parse_str(raw) {
        Ok(id) => EnvRef::Id(id),
        Err(_) => EnvRef::Name(raw.to_string()),
    }
}

/// Resolve an `id` parameter that may be an environment UUID or the
/// caller's environment name. Name lookups are owner-scoped by
/// construction; the caller re-checks ownership for the UUID path.
async fn resolve_environment(
    state: &AppState,
    caller: &Caller,
    raw: &str,
) -> Result<EnvironmentRow, Error> {
    let lookup = |row: Option<EnvironmentRow>| {
        row.ok_or_else(|| Error::new(ENV_NOT_FOUND, format!("environment {raw} not found")))
    };
    match parse_env_ref(raw) {
        EnvRef::Id(id) => lookup(signet_db::get_environment(&state.pool, id).await.map_err(
            |e| {
                tracing::error!(error = %e, "environment lookup failed");
                Error::new(INTERNAL_ERROR, "environment lookup failed")
            },
        )?),
        EnvRef::Name(name) => lookup(
            signet_db::get_environment_by_name(&state.pool, &caller.npub, &name)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "environment lookup failed");
                    Error::new(INTERNAL_ERROR, "environment lookup failed")
                })?,
        ),
    }
}

/// Lightning validation: names unique per env; only lnd is
/// buildable in v1 — the rest of the enum exists in the type space only.
fn validate_lightning(instances: &[signet_core::env::LightningInstance]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for instance in instances {
        // Names become k8s resource names: dns-1123 label, max 63 chars.
        let name = instance.name.as_str();
        let dns_safe = !name.is_empty()
            && name.len() <= 63
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !dns_safe {
            return Err(format!(
                "lightning instance name `{name}` must be a dns label (lowercase alphanumerics and '-', max 63)"
            ));
        }
        if !seen.insert(name) {
            return Err(format!("duplicate lightning instance name `{name}`"));
        }
        if !matches!(
            instance.implementation,
            signet_core::env::LightningImplementation::Lnd
        ) {
            return Err(format!(
                "lightning implementation `{}` is not available (only lnd is supported)",
                instance.implementation.as_str()
            ));
        }
    }
    Ok(())
}

fn resolve_expires_at(
    ttl_secs: Option<i64>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    match ttl_secs {
        None => Ok(None),
        Some(s) if s <= 0 => Err("ttl_secs must be positive".to_string()),
        Some(s) => chrono::TimeDelta::try_seconds(s)
            .and_then(|d| now.checked_add_signed(d))
            .map(Some)
            .ok_or_else(|| "ttl_secs out of range".to_string()),
    }
}

#[derive(Deserialize)]
struct FaucetParams {
    id: String,
    address: String,
    amount_sat: u64,
}

async fn environment_create(state: &AppState, id: Id, caller: Caller, params: Value) -> Response {
    let params: CreateParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };

    let images = match signet_orchestrator::resolve_images(&state.catalog, &params.versions) {
        Ok(images) => images,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };
    let expires_at = match resolve_expires_at(params.ttl_secs, Utc::now()) {
        Ok(expires_at) => expires_at,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e)),
    };
    let versions_json = serde_json::to_value(&images).ok();

    // A name held by a dead record (destroyed or expired — cluster
    // state already gone) is freed for reuse; live names stay taken.
    match signet_db::get_environment_by_name(&state.pool, &caller.npub, &params.name).await {
        Ok(Some(row)) if matches!(row.status.as_str(), "destroyed" | "expired") => {
            if let Err(e) = signet_db::purge_environment(&state.pool, row.id).await {
                tracing::error!(error = %e, "stale record purge failed");
                return Response::error(
                    Some(id),
                    Error::new(INTERNAL_ERROR, "stale record cleanup failed"),
                );
            }
        }
        Ok(Some(_)) => {
            return Response::error(
                Some(id),
                Error::new(
                    INVALID_PARAMS,
                    format!("environment name `{}` already in use", params.name),
                ),
            );
        }
        Ok(None) => {}
        Err(e) => {
            tracing::error!(error = %e, "environment name lookup failed");
            return Response::error(Some(id), Error::new(INTERNAL_ERROR, "name lookup failed"));
        }
    }

    if let Err(message) = validate_lightning(&params.components.lightning) {
        return Response::error(Some(id), Error::new(INVALID_PARAMS, message));
    }

    // Lightning needs only the chain backend; electrs stays opt-in for
    // wallets that track on-chain state over Electrum.
    let indexer = params.components.indexer;

    let env_id = Uuid::now_v7();
    let key = signet_signer::generate_key();
    let rpc_user = "signet".to_string();
    let rpc_password = Uuid::new_v4().simple().to_string();
    let lnd_wallet_password = Uuid::new_v4().simple().to_string();

    let secrets = EnvSecrets {
        signer_wif: key.wif,
        signer_pubkey: key.pubkey,
        signet_challenge: key.challenge.clone(),
        rpc_user: rpc_user.clone(),
        rpc_password: rpc_password.clone(),
        lnd_wallet_password: lnd_wallet_password.clone(),
    };

    let rpc_endpoint = format!("{}/env/{env_id}/rpc", state.public_url);
    let explorer_endpoint = params
        .components
        .explorer
        .then(|| state.orchestrator.explorer_url(&short_id(env_id)));
    let electrs_requested = indexer || params.components.explorer;
    let electrum_port = if electrs_requested {
        match signet_db::next_electrum_port(
            &state.pool,
            signet_orchestrator::ELECTRUM_PORT_BASE,
            signet_orchestrator::ELECTRUM_PORT_MAX,
        )
        .await
        {
            Ok(Some(port)) => Some(port),
            Ok(None) => {
                return Response::error(
                    Some(id),
                    Error::new(
                        INVALID_PARAMS,
                        format!(
                            "electrum port range exhausted ({}-{})",
                            signet_orchestrator::ELECTRUM_PORT_BASE,
                            signet_orchestrator::ELECTRUM_PORT_MAX,
                        ),
                    ),
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "electrum port allocation failed");
                return Response::error(
                    Some(id),
                    Error::new(INTERNAL_ERROR, "port allocation failed"),
                );
            }
        }
    } else {
        None
    };
    let indexer_endpoint = electrum_port.map(|p| format!("tcp://localhost:{p}"));
    let row = signet_db::NewEnvironment {
        id: env_id,
        name: &params.name,
        npub_owner: &caller.npub,
        block_policy: policy_as_str(params.block_policy.unwrap_or(BlockPolicy::Interval30s)),
        signet_challenge: &key.challenge,
        component_explorer: params.components.explorer,
        component_indexer: indexer,
        component_faucet: params.components.faucet,
        rpc_endpoint: &rpc_endpoint,
        explorer_endpoint: explorer_endpoint.as_deref(),
        indexer_endpoint: indexer_endpoint.as_deref(),
        electrum_port,
        ttl_secs: params.ttl_secs,
        expires_at,
        versions: versions_json,
    };

    let row = match signet_db::create_environment(&state.pool, &row).await {
        Ok(row) => row,
        Err(e) if signet_db::is_unique_violation(&e) => {
            return Response::error(
                Some(id),
                Error::new(
                    INVALID_PARAMS,
                    format!("environment name `{}` already in use", params.name),
                ),
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "environment insert failed");
            return Response::error(
                Some(id),
                Error::new(INTERNAL_ERROR, "environment persist failed"),
            );
        }
    };

    let mut lightning: Vec<(signet_core::env::LightningInstance, i32)> = Vec::new();
    if !params.components.lightning.is_empty() {
        match signet_db::next_lightning_ports(
            &state.pool,
            signet_orchestrator::LIGHTNING_PORT_BASE,
            signet_orchestrator::LIGHTNING_PORT_MAX,
            params.components.lightning.len(),
        )
        .await
        {
            Ok(ports) => {
                lightning = params
                    .components
                    .lightning
                    .iter()
                    .cloned()
                    .zip(ports)
                    .collect();
            }
            Err(e) => {
                tracing::error!(error = %e, "lightning port allocation failed");
                return Response::error(
                    Some(id),
                    Error::new(
                        INVALID_PARAMS,
                        format!(
                            "lightning port pool exhausted ({}-{})",
                            signet_orchestrator::LIGHTNING_PORT_BASE,
                            signet_orchestrator::LIGHTNING_PORT_MAX,
                        ),
                    ),
                );
            }
        }
    }

    let provision = signet_orchestrator::ProvisionParams {
        env_id: &short_id(env_id),
        secrets: &secrets,
        images: &images,
        components: EnvComponents {
            indexer,
            faucet: params.components.faucet,
            explorer: params.components.explorer,
            lightning: params.components.lightning.clone(),
        },
        electrum_port: electrum_port.map(|p| p as u16),
        lightning: &lightning,
        expires_at,
    };
    if let Err(e) = state.orchestrator.create_environment(&provision).await {
        tracing::error!(error = %e, "environment provisioning failed");
        // A failed create leaves a half-provisioned namespace behind;
        // tear it down best-effort so nothing keeps running unmanaged.
        if let Err(teardown) = state
            .orchestrator
            .destroy_environment(&short_id(env_id))
            .await
        {
            tracing::error!(error = %teardown, "namespace cleanup after failed provisioning failed");
        }
        let _ = signet_db::set_environment_status(&state.pool, env_id, "destroyed").await;
        return Response::error(
            Some(id),
            Error::new(INTERNAL_ERROR, "environment provisioning failed"),
        );
    }

    if let Err(e) = signet_db::set_lightning_instances(&state.pool, env_id, &lightning).await {
        tracing::error!(error = %e, "lightning instance persist failed");
    }

    spawn_lightning_mesh(
        state,
        env_id,
        lightning
            .iter()
            .map(|(instance, _)| instance.name.clone())
            .collect(),
    );

    Response::result(
        Some(id),
        serde_json::to_value(bundle_for(
            &row,
            EnvStatus::Provisioning,
            Some(format!("{rpc_user}:{rpc_password}")),
            BTreeMap::new(),
        ))
        .expect("bundle serializes"),
    )
}

async fn environment_get(state: &AppState, id: Id, caller: Caller, params: Value) -> Response {
    let params: IdParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };

    let mut row = match resolve_environment(state, &caller, &params.id).await {
        Ok(row) => row,
        Err(e) => return Response::error(Some(id), e),
    };

    if row.npub_owner != caller.npub {
        return Response::error(
            Some(id),
            Error::new(FORBIDDEN, "caller is not the environment owner"),
        );
    }

    reconcile_reaped(state, &mut row).await;

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

    bundle_response(state, id, &row, status).await
}

/// Assemble and serialize the full bundle for one row: live rpc
/// credentials plus per-instance lightning entries.
async fn bundle_response(
    state: &AppState,
    id: Id,
    row: &EnvironmentRow,
    status: EnvStatus,
) -> Response {
    let rpc_auth = state
        .orchestrator
        .rpc_credentials(&short_id(row.id))
        .await
        .ok()
        .map(|(u, p)| format!("{u}:{p}"));
    let lightning = lightning_connections(state, row.id, status).await;
    Response::result(
        Some(id),
        serde_json::to_value(bundle_for(row, status, rpc_auth, lightning))
            .expect("bundle serializes"),
    )
}

/// Envs are isolated chains: public signet seeds can never peer with
/// them (the p2p magic derives from the per-env challenge), so sibling
/// instances are each other's only peers. Wallets come up after
/// provisioning returns, so mesh in the background: retry until every
/// instance answers getinfo, then cross-connect the pairs.
fn spawn_lightning_mesh(state: &AppState, env_id: Uuid, names: Vec<String>) {
    if names.len() < 2 {
        return;
    }
    let orchestrator = state.orchestrator.clone();
    let short = short_id(env_id);
    tokio::spawn(async move {
        const ATTEMPTS: u64 = 80;
        const INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
        for _ in 0..ATTEMPTS {
            match orchestrator.mesh_lightning_peers(&short, &names).await {
                Ok(initiated) => {
                    tracing::info!(env = %short, initiated, "lightning instances peered");
                    return;
                }
                Err(e) => {
                    tracing::debug!(error = %e, env = %short, "lightning mesh not ready; retrying")
                }
            }
            tokio::time::sleep(INTERVAL).await;
        }
        tracing::warn!(env = %short, "gave up peering lightning instances");
    });
}

/// Per-instance connection entries for a bundle. An entry appears only
/// once its pod serves both macaroon and TLS cert; anything shorter is
/// "not ready yet" and omitted.
async fn lightning_connections(
    state: &AppState,
    env_id: Uuid,
    status: EnvStatus,
) -> BTreeMap<String, LightningConnection> {
    let mut connections = BTreeMap::new();
    if status != EnvStatus::Ready {
        return connections;
    }
    let instances = match signet_db::get_lightning_instances(&state.pool, env_id).await {
        Ok(instances) => instances,
        Err(e) => {
            tracing::warn!(error = %e, "lightning instance lookup failed");
            return connections;
        }
    };
    let short = short_id(env_id);
    for instance in instances {
        let Some(port) = instance.port else {
            tracing::warn!(env = %env_id, instance = %instance.name, "instance has no port; omitting");
            return connections;
        };
        let connection = async {
            let macaroon = state
                .orchestrator
                .lightning_admin_macaroon(&short, &instance.name)
                .await?;
            let tls_ca = state
                .orchestrator
                .lightning_tls_ca(&short, &instance.name)
                .await?;
            let node_id = state
                .orchestrator
                .lightning_node_id(&short, &instance.name)
                .await?;
            Ok::<_, signet_orchestrator::OrchestrateError>(LightningConnection {
                implementation: instance.implementation.clone(),
                node_id: node_id.clone(),
                node_uri: state.orchestrator.lightning_node_uri(port, &node_id),
                rest_url: state.orchestrator.lightning_rest_url(port),
                grpc_url: state.orchestrator.lightning_grpc_url(port),
                tls_cert: tls_ca,
                auth: signet_core::bundle::BundleAuth {
                    kind: "macaroon".to_string(),
                    value: macaroon,
                },
            })
        };
        match connection.await {
            Ok(connection) => {
                connections.insert(instance.name.clone(), connection);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    env = %env_id,
                    instance = %instance.name,
                    "lightning connection unavailable; omitting instance"
                );
            }
        }
    }
    connections
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateParams {
    id: String,
    components: UpdateComponents,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateComponents {
    lightning: Vec<signet_core::env::LightningInstance>,
}

/// v1 update scope: add lightning instances to a ready environment.
/// Removals, ttl changes, and other component changes are rejected —
/// the full update surface lands later.
async fn environment_update(state: &AppState, id: Id, caller: Caller, params: Value) -> Response {
    let params: UpdateParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };

    let mut row = match resolve_environment(state, &caller, &params.id).await {
        Ok(row) => row,
        Err(e) => return Response::error(Some(id), e),
    };

    if row.npub_owner != caller.npub {
        return Response::error(
            Some(id),
            Error::new(FORBIDDEN, "caller is not the environment owner"),
        );
    }

    reconcile_reaped(state, &mut row).await;

    if row.status != "ready" {
        return Response::error(
            Some(id),
            Error::new(
                INVALID_PARAMS,
                format!(
                    "environment `{}` is {} — only ready environments can be updated",
                    row.name, row.status
                ),
            ),
        );
    }

    if let Err(message) = validate_lightning(&params.components.lightning) {
        return Response::error(Some(id), Error::new(INVALID_PARAMS, message));
    }

    let existing = match signet_db::get_lightning_instances(&state.pool, row.id).await {
        Ok(existing) => existing,
        Err(e) => {
            tracing::error!(error = %e, "lightning instance lookup failed");
            return Response::error(
                Some(id),
                Error::new(INTERNAL_ERROR, "lightning instance lookup failed"),
            );
        }
    };
    for instance in &existing {
        if !params
            .components
            .lightning
            .iter()
            .any(|p| p.name == instance.name)
        {
            return Response::error(
                Some(id),
                Error::new(
                    INVALID_PARAMS,
                    format!(
                        "removing lightning instance `{}` is not supported",
                        instance.name
                    ),
                ),
            );
        }
    }
    let additions: Vec<signet_core::env::LightningInstance> = params
        .components
        .lightning
        .iter()
        .filter(|p| !existing.iter().any(|e| e.name == p.name))
        .cloned()
        .collect();

    if additions.is_empty() {
        return bundle_response(state, id, &row, EnvStatus::Ready).await;
    }

    let images: BTreeMap<String, String> = match row.versions.as_ref() {
        Some(versions) => match serde_json::from_value(versions.clone()) {
            Ok(images) => images,
            Err(e) => {
                tracing::error!(error = %e, "stored version map unreadable");
                return Response::error(
                    Some(id),
                    Error::new(INTERNAL_ERROR, "environment update failed"),
                );
            }
        },
        None => match signet_orchestrator::resolve_images(&state.catalog, &None) {
            Ok(images) => images,
            Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
        },
    };

    let mut lightning: Vec<(signet_core::env::LightningInstance, i32)> = existing
        .iter()
        .filter_map(|e| {
            params
                .components
                .lightning
                .iter()
                .find(|p| p.name == e.name)
                .map(|p| (p.clone(), e.port.unwrap_or_default()))
        })
        .collect();
    let addition_pairs: Vec<(signet_core::env::LightningInstance, i32)> = if additions.is_empty() {
        Vec::new()
    } else {
        match signet_db::next_lightning_ports(
            &state.pool,
            signet_orchestrator::LIGHTNING_PORT_BASE,
            signet_orchestrator::LIGHTNING_PORT_MAX,
            additions.len(),
        )
        .await
        {
            Ok(ports) => additions.iter().cloned().zip(ports).collect(),
            Err(e) => {
                tracing::error!(error = %e, "lightning port allocation failed");
                return Response::error(
                    Some(id),
                    Error::new(
                        INVALID_PARAMS,
                        format!(
                            "lightning port pool exhausted ({}-{})",
                            signet_orchestrator::LIGHTNING_PORT_BASE,
                            signet_orchestrator::LIGHTNING_PORT_MAX,
                        ),
                    ),
                );
            }
        }
    };
    lightning.extend(addition_pairs.iter().cloned());
    let wallet_password = Uuid::new_v4().simple().to_string();
    if let Err(e) = state
        .orchestrator
        .add_lightning_instances(
            &short_id(row.id),
            &addition_pairs,
            &images,
            &wallet_password,
        )
        .await
    {
        tracing::error!(error = %e, "lightning provisioning failed");
        return Response::error(
            Some(id),
            Error::new(INTERNAL_ERROR, "environment update failed"),
        );
    }

    if let Err(e) = signet_db::set_lightning_instances(&state.pool, row.id, &lightning).await {
        tracing::error!(error = %e, "lightning instance persist failed");
    }

    spawn_lightning_mesh(
        state,
        row.id,
        lightning
            .iter()
            .map(|(instance, _)| instance.name.clone())
            .collect(),
    );

    bundle_response(state, id, &row, EnvStatus::Ready).await
}

/// Converge a row whose namespace the reaper (or anything else) removed:
/// a live-looking status with no namespace is `expired`. Best effort — a
/// failed existence check leaves the row untouched.
async fn reconcile_reaped(state: &AppState, row: &mut EnvironmentRow) {
    if !matches!(row.status.as_str(), "provisioning" | "ready" | "stopped") {
        return;
    }
    match state
        .orchestrator
        .environment_exists(&short_id(row.id))
        .await
    {
        Ok(false) => {
            tracing::warn!(env_id = %row.id, "environment namespace is gone; marking expired");
            if let Err(e) = signet_db::set_environment_status(&state.pool, row.id, "expired").await
            {
                tracing::error!(error = %e, "expired status update failed");
            }
            row.status = "expired".to_string();
        }
        Ok(true) => {}
        Err(e) => tracing::warn!(error = %e, "environment existence check failed"),
    }
}

async fn environment_list(state: &AppState, id: Id, caller: Caller) -> Response {
    let rows = match signet_db::list_environments(&state.pool, &caller.npub).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "environment list failed");
            return Response::error(
                Some(id),
                Error::new(INTERNAL_ERROR, "environment list failed"),
            );
        }
    };
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "id": row.id,
                "name": row.name,
                "status": status_as_env(&row.status),
                "created_at": row.created_at,
                "expires_at": row.expires_at,
            })
        })
        .collect();
    Response::result(Some(id), serde_json::Value::Array(items))
}

async fn environment_stop(state: &AppState, id: Id, caller: Caller, params: Value) -> Response {
    lifecycle_transition(state, id, caller, params, Lifecycle::Stop).await
}

async fn environment_start(state: &AppState, id: Id, caller: Caller, params: Value) -> Response {
    lifecycle_transition(state, id, caller, params, Lifecycle::Start).await
}

enum Lifecycle {
    Stop,
    Start,
}

impl Lifecycle {
    fn required_status(&self) -> &'static str {
        match self {
            Lifecycle::Stop => "ready",
            Lifecycle::Start => "stopped",
        }
    }

    fn replicas(&self) -> i32 {
        match self {
            Lifecycle::Stop => 0,
            Lifecycle::Start => 1,
        }
    }

    fn result_status(&self) -> &'static str {
        match self {
            Lifecycle::Stop => "stopped",
            // get flips provisioning -> ready once the workloads pass readiness
            Lifecycle::Start => "provisioning",
        }
    }
}

async fn lifecycle_transition(
    state: &AppState,
    id: Id,
    caller: Caller,
    params: Value,
    transition: Lifecycle,
) -> Response {
    let params: IdParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };

    let mut row = match resolve_environment(state, &caller, &params.id).await {
        Ok(row) => row,
        Err(e) => return Response::error(Some(id), e),
    };

    if row.npub_owner != caller.npub {
        return Response::error(
            Some(id),
            Error::new(FORBIDDEN, "caller is not the environment owner"),
        );
    }

    reconcile_reaped(state, &mut row).await;

    if row.status != transition.required_status() {
        return Response::error(
            Some(id),
            Error::new(
                CHAIN_ERROR,
                format!(
                    "environment is {} (status: {})",
                    if matches!(transition, Lifecycle::Stop) {
                        "not ready"
                    } else {
                        "not stopped"
                    },
                    row.status
                ),
            ),
        );
    }

    if let Err(e) = state
        .orchestrator
        .set_workload_replicas(&short_id(row.id), transition.replicas())
        .await
    {
        tracing::error!(error = %e, "workload scale failed");
        return Response::error(
            Some(id),
            Error::new(INTERNAL_ERROR, "workload scale failed"),
        );
    }

    if let Err(e) =
        signet_db::set_environment_status(&state.pool, row.id, transition.result_status()).await
    {
        tracing::error!(error = %e, "environment status update failed");
    }

    Response::result(
        Some(id),
        serde_json::json!({ "id": row.id, "status": transition.result_status() }),
    )
}

async fn environment_destroy(state: &AppState, id: Id, caller: Caller, params: Value) -> Response {
    let params: IdParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };

    let row = match resolve_environment(state, &caller, &params.id).await {
        Ok(row) => row,
        Err(e) => return Response::error(Some(id), e),
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

/// Remove one environment's record. Only records whose cluster state is
/// gone (destroyed or expired) can be deleted; the caller must own it.
async fn environment_delete(state: &AppState, id: Id, caller: Caller, params: Value) -> Response {
    let params: IdParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };

    let row = match resolve_environment(state, &caller, &params.id).await {
        Ok(row) => row,
        Err(e) => return Response::error(Some(id), e),
    };

    if row.npub_owner != caller.npub {
        return Response::error(
            Some(id),
            Error::new(FORBIDDEN, "caller is not the environment owner"),
        );
    }

    if !matches!(row.status.as_str(), "destroyed" | "expired") {
        return Response::error(
            Some(id),
            Error::new(
                INVALID_PARAMS,
                "environment must be destroyed or expired before its record can be deleted",
            ),
        );
    }

    match signet_db::purge_environment(&state.pool, row.id).await {
        Ok(true) => Response::result(Some(id), serde_json::json!({ "deleted": true })),
        Ok(false) => Response::error(
            Some(id),
            Error::new(INVALID_PARAMS, "environment not found"),
        ),
        Err(e) => {
            tracing::error!(error = %e, "environment delete failed");
            Response::error(
                Some(id),
                Error::new(INTERNAL_ERROR, "environment delete failed"),
            )
        }
    }
}

async fn environment_faucet(state: &AppState, id: Id, caller: Caller, params: Value) -> Response {
    let params: FaucetParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(Some(id), Error::new(INVALID_PARAMS, e.to_string())),
    };

    let row = match resolve_environment(state, &caller, &params.id).await {
        Ok(row) => row,
        Err(e) => return Response::error(Some(id), e),
    };

    if row.npub_owner != caller.npub {
        return Response::error(
            Some(id),
            Error::new(FORBIDDEN, "caller is not the environment owner"),
        );
    }

    if !row.component_faucet {
        return Response::error(
            Some(id),
            Error::new(
                NOT_AVAILABLE_IN_PHASE,
                "environment has no faucet component",
            ),
        );
    }

    if row.status != "ready" {
        return Response::error(
            Some(id),
            Error::new(
                FAUCET_FAILED,
                format!("environment is not ready (status: {})", row.status),
            ),
        );
    }

    if let Err(message) = signet_faucet::validate_signet_address(&params.address) {
        return Response::error(Some(id), Error::new(INVALID_ADDRESS, message));
    }

    match state
        .orchestrator
        .faucet_fund(&short_id(row.id), &params.address, params.amount_sat)
        .await
    {
        Ok(txid) => Response::result(Some(id), serde_json::json!({ "txid": txid })),
        Err(e) => {
            tracing::error!(error = %e, "faucet funding failed");
            Response::error(Some(id), Error::new(FAUCET_FAILED, "faucet funding failed"))
        }
    }
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
        "stopped" => EnvStatus::Stopped,
        "expired" => EnvStatus::Expired,
        "destroyed" => EnvStatus::Destroyed,
        _ => EnvStatus::Provisioning,
    }
}

fn bundle_for(
    row: &EnvironmentRow,
    status: EnvStatus,
    rpc_auth: Option<String>,
    lightning: BTreeMap<String, LightningConnection>,
) -> ConnectionBundle {
    ConnectionBundle {
        environment_id: row.id.to_string(),
        status,
        rpc_url: row.rpc_endpoint.clone(),
        rpc_auth,
        zmq_url: None,
        indexer_endpoint: row.indexer_endpoint.clone(),
        explorer_url: row.explorer_endpoint.clone(),
        faucet_url: row.faucet_endpoint.clone(),
        lightning,
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

    const RPC_URL: &str = "http://localhost:8081/api/rpc";

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
        verify_nip98(
            header.strip_prefix("Nostr ").unwrap(),
            RPC_URL,
            "POST",
            Some(signet_nostr::DEFAULT_MAX_AGE),
        )
    }

    fn headers_with_host(host: &str, forwarded_proto: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, host.parse().unwrap());
        if let Some(proto) = forwarded_proto {
            headers.insert("x-forwarded-proto", proto.parse().unwrap());
        }
        headers
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
    fn a_non_public_host_verifies_when_it_is_the_received_url() {
        // Regression: bootstrap --host advertises a non-localhost public
        // URL while the local caller still signs the localhost URL it
        // posts to. Verification follows the received URL, not the
        // advertised one.
        let keys = Keys::parse(&format!("{:064x}", 7)).unwrap();
        let lan_url = "http://192.168.100.2:8081/api/rpc";
        let header = auth_header(&keys, lan_url, "POST");
        let caller = verify_nip98(
            header.strip_prefix("Nostr ").unwrap(),
            lan_url,
            "POST",
            Some(signet_nostr::DEFAULT_MAX_AGE),
        )
        .unwrap();
        assert_eq!(caller.npub, keys.public_key().to_bech32().unwrap());
    }

    #[test]
    fn request_url_is_scheme_host_and_path() {
        let headers = headers_with_host("localhost:8081", None);
        assert_eq!(
            request_url(&headers, "/api/rpc"),
            "http://localhost:8081/api/rpc"
        );
    }

    #[test]
    fn request_url_trusts_the_first_forwarded_proto() {
        let headers = headers_with_host("api.example", Some("https,http"));
        assert_eq!(
            request_url(&headers, "/api/rpc"),
            "https://api.example/api/rpc"
        );
    }

    #[test]
    fn nip98_url_mismatch_is_unauthenticated() {
        let keys = Keys::parse(&format!("{:064x}", 7)).unwrap();
        let header = auth_header(&keys, "http://evil.example/api/rpc", "POST");
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
    fn parse_env_ref_distinguishes_uuid_from_name() {
        let id = Uuid::now_v7();
        assert!(matches!(
            parse_env_ref(&id.to_string()),
            EnvRef::Id(parsed) if parsed == id
        ));
        assert!(matches!(
            parse_env_ref("acme-staging"),
            EnvRef::Name(name) if name == "acme-staging"
        ));
    }

    fn instance(
        name: &str,
        implementation: signet_core::env::LightningImplementation,
    ) -> signet_core::env::LightningInstance {
        signet_core::env::LightningInstance {
            name: name.into(),
            implementation,
        }
    }

    #[test]
    fn validate_lightning_accepts_unique_lnd_instances() {
        assert!(
            validate_lightning(&[
                instance("ln-alice", signet_core::env::LightningImplementation::Lnd),
                instance("ln-bob", signet_core::env::LightningImplementation::Lnd),
            ])
            .is_ok()
        );
        assert!(validate_lightning(&[]).is_ok());
    }

    #[test]
    fn validate_lightning_rejects_duplicates_and_unsupported_implementations() {
        assert!(
            validate_lightning(&[
                instance("ln-alice", signet_core::env::LightningImplementation::Lnd),
                instance("ln-alice", signet_core::env::LightningImplementation::Lnd),
            ])
            .is_err()
        );
        assert!(
            validate_lightning(&[instance(
                "ln-cln",
                signet_core::env::LightningImplementation::Cln
            )])
            .is_err()
        );
    }

    #[test]
    fn validate_lightning_rejects_names_that_break_k8s_resources() {
        for bad in [
            "",
            "Upper",
            "has_underscore",
            "-start",
            "a".repeat(64).as_str(),
        ] {
            assert!(
                validate_lightning(&[instance(
                    bad,
                    signet_core::env::LightningImplementation::Lnd
                )])
                .is_err(),
                "{bad} must be rejected"
            );
        }
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
    fn resolve_expires_at_validates_ttl() {
        let now = Utc::now();
        assert!(resolve_expires_at(None, now).unwrap().is_none());
        let expires = resolve_expires_at(Some(1200), now).unwrap().unwrap();
        assert_eq!(expires, now + chrono::Duration::seconds(1200));
        assert!(resolve_expires_at(Some(0), now).is_err());
        assert!(resolve_expires_at(Some(-5), now).is_err());
        assert!(resolve_expires_at(Some(i64::MAX), now).is_err());
    }

    #[test]
    fn faucet_params_parse() {
        let env_id = Uuid::now_v7();
        let params: FaucetParams = serde_json::from_value(serde_json::json!({
            "id": env_id,
            "address": "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx",
            "amount_sat": 1_000
        }))
        .unwrap();
        assert_eq!(params.id, env_id.to_string());
        assert_eq!(params.amount_sat, 1_000);

        for incomplete in [
            serde_json::json!({ "address": "tb1q", "amount_sat": 1 }),
            serde_json::json!({ "id": env_id, "amount_sat": 1 }),
            serde_json::json!({ "id": env_id, "address": "tb1q" }),
        ] {
            assert!(serde_json::from_value::<FaucetParams>(incomplete).is_err());
        }
    }

    #[test]
    fn invalid_signet_addresses_fail_validation() {
        for raw in [
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
            "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2",
            "not-an-address",
        ] {
            assert!(
                signet_faucet::validate_signet_address(raw).is_err(),
                "{raw}"
            );
        }
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
        let json = serde_json::to_value(bundle_for(
            &row,
            EnvStatus::Provisioning,
            None,
            BTreeMap::new(),
        ))
        .unwrap();
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
        let json = serde_json::to_value(bundle_for(&row, EnvStatus::Ready, None, BTreeMap::new()))
            .unwrap();
        assert_eq!(
            json["versions"]["bitcoind"],
            serde_json::json!("bitcoin/bitcoin:28.1")
        );
    }
}
