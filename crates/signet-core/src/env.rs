use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
pub struct CreateParams {
    pub name: String,
    #[serde(default)]
    pub block_policy: Option<BlockPolicy>,
    #[serde(default)]
    pub components: Components,
    #[serde(default)]
    pub versions: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub ttl_secs: Option<i64>,
}

#[derive(Debug, Clone, Default, JsonSchema, Serialize, Deserialize)]
pub struct Components {
    #[serde(default)]
    pub explorer: bool,
    #[serde(default)]
    pub indexer: bool,
    #[serde(default)]
    pub faucet: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub enum BlockPolicy {
    #[serde(rename = "interval_30s")]
    Interval30s,
    #[serde(rename = "on_demand")]
    OnDemand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvStatus {
    Provisioning,
    Ready,
    Stopped,
    Expired,
    Destroyed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub npub_owner: String,
    pub workspace_id: Option<String>,
    pub status: EnvStatus,
    pub block_policy: BlockPolicy,
    pub signet_challenge: String,
    pub rpc_endpoint: Option<String>,
    pub indexer_endpoint: Option<String>,
    pub explorer_endpoint: Option<String>,
    pub faucet_endpoint: Option<String>,
    pub ttl_secs: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub current_snapshot_id: Option<String>,
}
