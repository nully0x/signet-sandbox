use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use signet_db::{ApiTokenRow, PgPool};
use signet_nostr::{ApiToken, DEFAULT_MAX_AGE, verify_nip98_header};
use signet_rpc::envelope::{Id, Request, Response};
use signet_rpc::error::{Error, INVALID_REQUEST, PARSE_ERROR, UNAUTHENTICATED};
use std::time::Duration;

const NIP98_MAX_AGE: Duration = DEFAULT_MAX_AGE;

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
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

pub fn router(pool: PgPool, public_url: String) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/rpc", post(dispatch))
        .with_state(AppState { pool, public_url })
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

    let response = handle(caller, request).await;
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
    let row: Option<ApiTokenRow> = signet_db::find_api_token(pool, token.hash().as_hex())
        .await
        .map_err(|_| Error::new(signet_rpc::error::INTERNAL_ERROR, "token lookup failed"))?;

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

async fn handle(_caller: Caller, request: Request) -> Response {
    let id = request.id.clone().unwrap_or(Id::Null);
    match request.method.as_str() {
        "environment.create" | "environment.get" | "environment.destroy" | "environment.faucet" => {
            Response::error(
                Some(id),
                Error::new(
                    signet_rpc::error::NOT_AVAILABLE_IN_PHASE,
                    format!("method {} not yet implemented", request.method),
                ),
            )
        }
        _ => Response::error(Some(id), Error::method_not_found(&request.method)),
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
}
