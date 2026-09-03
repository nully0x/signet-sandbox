use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::env::{BlockPolicy, EnvStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightningConnection {
    pub implementation: String,
    pub rest_url: String,
    pub macaroon_or_rune: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionBundle {
    pub environment_id: String,
    pub status: EnvStatus,
    pub rpc_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_auth: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zmq_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexer_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explorer_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faucet_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lightning: Option<LightningConnection>,
    pub signet_challenge: String,
    pub block_policy: BlockPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_bundle() -> ConnectionBundle {
        ConnectionBundle {
            environment_id: "env_test".into(),
            status: EnvStatus::Ready,
            rpc_url: "https://example/env/rpc".into(),
            rpc_auth: Some("user:pass".into()),
            zmq_url: None,
            indexer_url: None,
            explorer_url: None,
            faucet_url: None,
            lightning: None,
            signet_challenge: "5121030000ae".into(),
            block_policy: BlockPolicy::Interval30s,
            versions: None,
            expires_at: None,
        }
    }

    #[test]
    fn disabled_components_are_omitted() {
        let json = serde_json::to_value(core_bundle()).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("environment_id"));
        assert!(obj.contains_key("rpc_url"));
        for absent in [
            "zmq_url",
            "indexer_url",
            "explorer_url",
            "faucet_url",
            "lightning",
            "expires_at",
            "tier",
        ] {
            assert!(!obj.contains_key(absent), "{absent} must be absent");
        }
    }

    #[test]
    fn enabled_components_are_included() {
        let mut b = core_bundle();
        b.explorer_url = Some("https://example/env/explorer".into());
        b.lightning = Some(LightningConnection {
            implementation: "lnd".into(),
            rest_url: "https://example/env/ln/rest".into(),
            macaroon_or_rune: "0201".into(),
        });
        let obj = serde_json::to_value(b).unwrap();
        let obj = obj.as_object().unwrap();
        assert!(obj.contains_key("explorer_url"));
        assert!(obj.contains_key("lightning"));
    }

    #[test]
    fn block_policy_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(BlockPolicy::Interval30s).unwrap(),
            serde_json::json!("interval_30s")
        );
        assert_eq!(
            serde_json::to_value(BlockPolicy::OnDemand).unwrap(),
            serde_json::json!("on_demand")
        );
    }

    #[test]
    fn versions_omitted_when_unset_and_echoed_when_set() {
        let json = serde_json::to_value(core_bundle()).unwrap();
        assert!(!json.as_object().unwrap().contains_key("versions"));

        let mut b = core_bundle();
        b.versions = Some(BTreeMap::from([(
            "bitcoind".to_string(),
            "bitcoin/bitcoin:29.4".to_string(),
        )]));
        let json = serde_json::to_value(b).unwrap();
        assert_eq!(
            json["versions"]["bitcoind"],
            serde_json::json!("bitcoin/bitcoin:29.4")
        );
    }
}
