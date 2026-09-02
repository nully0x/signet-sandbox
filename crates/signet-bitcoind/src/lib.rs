pub mod types;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;
pub use types::BlockTemplate;

#[derive(Debug, Error)]
pub enum RpcError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("rpc error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("unexpected response: {0}")]
    Unexpected(String),
}

pub type Result<T> = std::result::Result<T, RpcError>;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    url: String,
    user: String,
    password: String,
}

impl Client {
    pub fn new(
        url: impl Into<String>,
        user: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            url: url.into().trim_end_matches('/').to_string(),
            user: user.into(),
            password: password.into(),
        }
    }

    pub fn wallet(&self, name: &str) -> Self {
        Self {
            url: format!("{}/wallet/{name}", self.url),
            ..self.clone()
        }
    }

    pub async fn call_raw(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let response = self
            .http
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.password))
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let envelope: Value = response.json().await?;
        if let Some(error) = envelope.get("error")
            && !error.is_null()
        {
            return Err(RpcError::Rpc {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(-1),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
            });
        }
        envelope
            .get("result")
            .cloned()
            .ok_or_else(|| RpcError::Unexpected(envelope.to_string()))
    }

    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let value = self.call_raw(method, params).await?;
        serde_json::from_value(value).map_err(|e| RpcError::Unexpected(e.to_string()))
    }

    pub async fn get_block_template(&self) -> Result<BlockTemplate> {
        self.call(
            "getblocktemplate",
            json!([{ "rules": ["signet", "segwit"] }]),
        )
        .await
    }

    pub async fn submit_block(&self, hex: &str) -> Result<Option<String>> {
        self.call("submitblock", json!([hex])).await
    }

    pub async fn get_blockchain_info(&self) -> Result<types::BlockchainInfo> {
        self.call("getblockchaininfo", json!([])).await
    }

    pub async fn create_wallet(&self, name: &str) -> Result<Value> {
        self.call("createwallet", json!([name])).await
    }

    pub async fn load_wallet(&self, name: &str) -> Result<Value> {
        self.call("loadwallet", json!([name])).await
    }

    pub async fn get_new_address(&self, label: &str) -> Result<String> {
        self.call("getnewaddress", json!([label, "bech32"])).await
    }

    pub async fn get_address_info(&self, address: &str) -> Result<types::AddressInfo> {
        self.call("getaddressinfo", json!([address])).await
    }

    pub async fn send_to_address(&self, address: &str, amount_btc: f64) -> Result<String> {
        self.call("sendtoaddress", json!([address, amount_btc]))
            .await
    }

    pub async fn get_wallet_info(&self) -> Result<Value> {
        self.call("getwalletinfo", json!([])).await
    }
}
