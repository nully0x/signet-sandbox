use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

pub const NOT_AVAILABLE_IN_PHASE: i64 = -32001;
pub const UNAUTHENTICATED: i64 = -32002;
pub const RATE_LIMITED: i64 = -32003;
pub const FORBIDDEN: i64 = -32004;
pub const ENV_NOT_FOUND: i64 = -32010;
pub const INVALID_ADDRESS: i64 = -32020;
pub const FAUCET_FAILED: i64 = -32021;
pub const CHAIN_ERROR: i64 = -32030;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Error {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("method not found: {method}"))
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCode {
    NotAvailableInPhase,
    Unauthenticated,
    RateLimited,
    Forbidden,
    EnvNotFound,
    InvalidAddress,
    FaucetFailed,
    ChainError,
}

impl AppCode {
    pub fn code(self) -> i64 {
        match self {
            AppCode::NotAvailableInPhase => NOT_AVAILABLE_IN_PHASE,
            AppCode::Unauthenticated => UNAUTHENTICATED,
            AppCode::RateLimited => RATE_LIMITED,
            AppCode::Forbidden => FORBIDDEN,
            AppCode::EnvNotFound => ENV_NOT_FOUND,
            AppCode::InvalidAddress => INVALID_ADDRESS,
            AppCode::FaucetFailed => FAUCET_FAILED,
            AppCode::ChainError => CHAIN_ERROR,
        }
    }
}
