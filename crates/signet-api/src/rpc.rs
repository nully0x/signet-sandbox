use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use signet_db::PgPool;

use signet_rpc::envelope::{Id, Request, Response};
use signet_rpc::error::{Error, INVALID_REQUEST, PARSE_ERROR};

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/rpc", post(dispatch))
        .with_state(pool)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn dispatch(State(_pool): State<PgPool>, body: axum::body::Bytes) -> impl IntoResponse {
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

    let response = handle(request).await;
    (StatusCode::OK, Json(response))
}

async fn handle(request: Request) -> Response {
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
