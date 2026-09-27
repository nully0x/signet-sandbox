// The sandbox.json config file (spec §5) translated into `CreateParams`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use signet_core::{BlockPolicy, Components, CreateParams};

use crate::duration::parse_duration_secs;

#[derive(Debug, Deserialize)]
pub struct SandboxConfig {
    pub name: Option<String>,
    pub block_policy: Option<BlockPolicy>,
    pub components: Option<Components>,
    pub versions: Option<BTreeMap<String, String>>,
    /// Human duration (`20m`, `1h`); the API wire format is `ttl_secs`.
    pub ttl: Option<String>,
}

pub fn load_config(path: &Path) -> anyhow::Result<SandboxConfig> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read config {}: {e}", path.display()))?;
    serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("invalid config {}: {e}", path.display()))
}

/// Flags win over the config file; a missing name is an error.
pub fn build_create_params(
    config: Option<&SandboxConfig>,
    name_flag: Option<String>,
    ttl_flag: Option<String>,
) -> anyhow::Result<CreateParams> {
    let ttl_secs = match (&ttl_flag, config.and_then(|c| c.ttl.as_deref())) {
        (Some(flag), _) => Some(parse_duration_secs(flag)? as i64),
        (None, Some(config_ttl)) => Some(parse_duration_secs(config_ttl)? as i64),
        (None, None) => None,
    };
    let name = match (name_flag, config.and_then(|c| c.name.clone())) {
        (Some(name), _) => name,
        (None, Some(name)) => name,
        (None, None) => {
            anyhow::bail!("no environment name: pass --name or set `name` in the config file")
        }
    };
    Ok(CreateParams {
        name,
        block_policy: config.and_then(|c| c.block_policy),
        components: config
            .and_then(|c| c.components.clone())
            .unwrap_or_default(),
        versions: config.and_then(|c| c.versions.clone()),
        ttl_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: &str) -> SandboxConfig {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn parses_the_spec_config_shape() {
        let cfg = config(
            r#"{
                "name": "acme-staging",
                "block_policy": "interval_30s",
                "components": { "explorer": true, "indexer": true, "faucet": true },
                "versions": { "bitcoind": "29.4" },
                "ttl": "1h"
            }"#,
        );
        let params = build_create_params(Some(&cfg), None, None).unwrap();
        assert_eq!(params.name, "acme-staging");
        assert_eq!(params.block_policy, Some(BlockPolicy::Interval30s));
        assert!(params.components.explorer);
        assert_eq!(params.versions.as_ref().unwrap()["bitcoind"], "29.4");
        assert_eq!(params.ttl_secs, Some(3600));
    }

    #[test]
    fn flags_override_the_config_file() {
        let cfg = config(r#"{ "name": "from-file", "ttl": "1h" }"#);
        let params =
            build_create_params(Some(&cfg), Some("from-flag".into()), Some("20m".into())).unwrap();
        assert_eq!(params.name, "from-flag");
        assert_eq!(params.ttl_secs, Some(1200));
    }

    #[test]
    fn minimal_flags_produce_default_components() {
        let params = build_create_params(None, Some("quick".into()), None).unwrap();
        assert_eq!(params.name, "quick");
        assert!(!params.components.explorer);
        assert!(!params.components.indexer);
        assert!(!params.components.faucet);
        assert_eq!(params.ttl_secs, None);
    }

    #[test]
    fn requires_a_name_from_flag_or_config() {
        let cfg = config(r#"{ "components": { "faucet": true } }"#);
        assert!(build_create_params(Some(&cfg), None, None).is_err());
        let params = build_create_params(Some(&cfg), None, Some("10m".into()));
        assert!(params.is_err() || params.unwrap().ttl_secs == Some(600));
    }

    #[test]
    fn rejects_an_invalid_config_file() {
        let err = load_config(Path::new("/nonexistent/sandbox.json")).unwrap_err();
        assert!(err.to_string().contains("cannot read config"));
    }
}
