use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use bitcoin::{Address, Network, address::NetworkUnchecked};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use signet_bitcoind::Client;

pub const SATS_PER_BTC: f64 = 100_000_000.0;

#[derive(Debug, Deserialize)]
pub struct FundRequest {
    pub address: String,
    pub amount_sat: u64,
}

#[derive(Debug, Serialize)]
pub struct FundResponse {
    pub txid: String,
}

#[derive(Clone)]
pub struct FaucetState {
    client: Client,
    wallet: String,
    max_sats: u64,
}

impl FaucetState {
    pub fn new(client: Client, wallet: impl Into<String>, max_sats: u64) -> Self {
        Self {
            client,
            wallet: wallet.into(),
            max_sats,
        }
    }
}

pub fn validate_signet_address(raw: &str) -> Result<Address, String> {
    let unchecked = raw
        .parse::<Address<NetworkUnchecked>>()
        .map_err(|e| format!("invalid address `{raw}`: {e}"))?;
    unchecked
        .require_network(Network::Signet)
        .map_err(|e| format!("address `{raw}` is not a signet address: {e}"))
}

pub fn check_amount(amount_sat: u64, max_sats: u64) -> Result<(), String> {
    if amount_sat == 0 {
        Err("amount_sat must be positive".to_string())
    } else if amount_sat > max_sats {
        Err(format!(
            "amount_sat {amount_sat} exceeds the per-request cap of {max_sats}"
        ))
    } else {
        Ok(())
    }
}

pub fn build_router(state: FaucetState) -> Router {
    Router::new()
        .route("/fund", post(fund))
        .route("/healthz", get(healthz))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

fn error_response(status: StatusCode, message: String) -> (StatusCode, Json<Value>) {
    (status, Json(serde_json::json!({ "error": message })))
}

async fn fund(
    State(state): State<FaucetState>,
    Json(request): Json<FundRequest>,
) -> Result<Json<FundResponse>, (StatusCode, Json<Value>)> {
    if let Err(message) = check_amount(request.amount_sat, state.max_sats) {
        return Err(error_response(StatusCode::BAD_REQUEST, message));
    }
    let address = match validate_signet_address(&request.address) {
        Ok(address) => address,
        Err(message) => return Err(error_response(StatusCode::BAD_REQUEST, message)),
    };

    let amount_btc = request.amount_sat as f64 / SATS_PER_BTC;
    match state
        .client
        .wallet(&state.wallet)
        .send_to_address(&address.to_string(), amount_btc)
        .await
    {
        Ok(txid) => Ok(Json(FundResponse { txid })),
        Err(e) => {
            tracing::warn!(error = %e, "faucet spend failed");
            Err(error_response(
                StatusCode::BAD_GATEWAY,
                format!("bitcoind rejected the spend: {e}"),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fund_request_parses_expected_shape() {
        let request: FundRequest = serde_json::from_value(serde_json::json!({
            "address": "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx",
            "amount_sat": 1_000
        }))
        .unwrap();
        assert_eq!(request.amount_sat, 1_000);
    }

    #[test]
    fn fund_request_requires_address_and_amount() {
        assert!(serde_json::from_value::<FundRequest>(serde_json::json!({})).is_err());
        assert!(
            serde_json::from_value::<FundRequest>(serde_json::json!({ "address": "tb1q" }))
                .is_err()
        );
    }

    #[test]
    fn signet_bech32_addresses_validate() {
        assert!(validate_signet_address("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx").is_ok());
    }

    #[test]
    fn non_signet_addresses_are_rejected() {
        for raw in [
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
            "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2",
            "not-an-address",
        ] {
            assert!(validate_signet_address(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn amount_must_be_positive_and_within_cap() {
        assert!(check_amount(1, 100).is_ok());
        assert!(check_amount(0, 100).is_err());
        assert!(check_amount(101, 100).is_err());
    }
}
