use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::env::{BlockPolicy, EnvStatus};

/// Per-instance credential: the `type` discriminator leaves room
/// for runes (CLN) and other schemes without a schema change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleAuth {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightningConnection {
    pub implementation: String,
    /// Hex identity public key; implementations disagree on the name
    /// (lnd `identity_pubkey`, CLN `id`), so the bundle fixes one.
    pub node_id: String,
    /// Peering URI `node_id@host:port`; the host port maps to p2p 9735.
    pub node_uri: String,
    pub rest_url: String,
    pub grpc_url: String,
    /// Instance TLS CA in PEM form. Public material; clients pin it as
    /// a root — the served leaf chains to it.
    pub tls_cert: String,
    pub auth: BundleAuth,
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
    pub indexer_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explorer_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faucet_url: Option<String>,
    /// Keyed by instance name; empty and omitted when the env
    /// has no Lightning instances or they are not ready yet.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lightning: BTreeMap<String, LightningConnection>,
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
            indexer_endpoint: None,
            explorer_url: None,
            faucet_url: None,
            lightning: BTreeMap::new(),
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
            "indexer_endpoint",
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
        b.lightning.insert(
            "ln-alice".to_string(),
            LightningConnection {
                implementation: "lnd".into(),
                node_id: "02aa55".into(),
                node_uri: "02aa55@example:21002".into(),
                rest_url: "https://example/env/ln/rest".into(),
                grpc_url: "https://example/env/ln/grpc".into(),
                tls_cert: "-----BEGIN CERTIFICATE-----".into(),
                auth: BundleAuth {
                    kind: "macaroon".into(),
                    value: "0201".into(),
                },
            },
        );
        let obj = serde_json::to_value(b).unwrap();
        let obj = obj.as_object().unwrap();
        assert!(obj.contains_key("explorer_url"));
        assert!(obj.contains_key("lightning"));
    }

    #[test]
    fn lightning_auth_uses_the_type_discriminator() {
        let mut b = core_bundle();
        b.lightning.insert(
            "ln-alice".to_string(),
            LightningConnection {
                implementation: "lnd".into(),
                node_id: "02aa55".into(),
                node_uri: "02aa55@example:21002".into(),
                rest_url: "https://example/ln".into(),
                grpc_url: "https://example/ln-grpc".into(),
                tls_cert: "-----BEGIN CERTIFICATE-----".into(),
                auth: BundleAuth {
                    kind: "macaroon".into(),
                    value: "0202".into(),
                },
            },
        );
        let json = serde_json::to_value(b).unwrap();
        let entry = &json["lightning"]["ln-alice"];
        assert_eq!(entry["implementation"], "lnd");
        assert_eq!(entry["node_id"], "02aa55");
        assert_eq!(entry["node_uri"], "02aa55@example:21002");
        assert_eq!(entry["auth"]["type"], "macaroon");
        assert_eq!(entry["auth"]["value"], "0202");
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
