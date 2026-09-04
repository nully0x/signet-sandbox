use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::{Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Secret, Service};
use kube::api::{Api, DeleteParams, ObjectMeta, PostParams};
use kube::{Client, Error};
use serde::Deserialize;
use serde_yaml::Value;

const BITCOIND_MANIFEST: &str = include_str!("../templates/bitcoind.yaml");
const SIGNER_MANIFEST: &str = include_str!("../templates/signer.yaml");
const FAUCET_MANIFEST: &str = include_str!("../templates/faucet.yaml");

const SECRET_NAME: &str = "signet-secrets";

const DEFAULT_BITCOIND_IMAGE: &str = "bitcoin/bitcoin:29.4";
const DEFAULT_SIGNER_IMAGE: &str = "signet-signer:dev";
const DEFAULT_FAUCET_IMAGE: &str = "signet-faucet:dev";

#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    #[error("unknown component `{0}` (expected bitcoind, electrs, explorer, or lnd)")]
    UnknownComponent(String),
    #[error("invalid image tag `{0}` (expected alphanumeric with . _ -)")]
    BadTag(String),
}

pub fn resolve_images(
    tags: &Option<BTreeMap<String, String>>,
) -> Result<BTreeMap<String, String>, VersionError> {
    let mut images = BTreeMap::from([
        ("bitcoind".to_string(), DEFAULT_BITCOIND_IMAGE.to_string()),
        ("signer".to_string(), DEFAULT_SIGNER_IMAGE.to_string()),
        ("faucet".to_string(), DEFAULT_FAUCET_IMAGE.to_string()),
    ]);
    let Some(tags) = tags else {
        return Ok(images);
    };
    for (component, tag) in tags {
        if !matches!(
            component.as_str(),
            "bitcoind" | "electrs" | "explorer" | "lnd"
        ) {
            return Err(VersionError::UnknownComponent(component.clone()));
        }
        if tag.is_empty()
            || tag.len() > 64
            || !tag
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(VersionError::BadTag(tag.clone()));
        }
        let repo = match component.as_str() {
            "bitcoind" => "bitcoin/bitcoin",
            "lnd" => "lightninglabs/lnd",
            "electrs" => "romanz/electrs",
            "explorer" => "bitcoinexplorer/btc-rpc-explorer",
            _ => unreachable!("validated above"),
        };
        images.insert(component.clone(), format!("{repo}:{tag}"));
    }
    Ok(images)
}

fn patch_images(
    containers: &mut [k8s_openapi::api::core::v1::Container],
    images: &BTreeMap<String, String>,
) {
    for container in containers {
        if let Some(image) = images.get(&container.name) {
            container.image = Some(image.clone());
        }
    }
}

pub fn namespace_for(env_id: &str) -> String {
    format!("env-{env_id}")
}

#[derive(Debug, Clone)]
pub struct EnvSecrets {
    pub signer_wif: String,
    pub signer_pubkey: String,
    pub signet_challenge: String,
    pub rpc_user: String,
    pub rpc_password: String,
}

#[derive(Debug, thiserror::Error)]
pub enum OrchestrateError {
    #[error("kubernetes api: {0}")]
    Kube(#[from] Error),
    #[error("manifest parse: {0}")]
    Manifest(#[from] serde_yaml::Error),
    #[error("manifest has no `kind`: {0}")]
    ManifestKind(String),
}

pub struct Orchestrator {
    client: Client,
}

impl Orchestrator {
    pub async fn connect() -> Result<Self, OrchestrateError> {
        Ok(Self {
            client: Client::try_default().await?,
        })
    }

    #[cfg(test)]
    pub fn from_client(client: Client) -> Self {
        Self { client }
    }

    pub async fn create_environment(
        &self,
        env_id: &str,
        secrets: &EnvSecrets,
        images: &BTreeMap<String, String>,
        faucet: bool,
    ) -> Result<(), OrchestrateError> {
        let ns = namespace_for(env_id);

        let namespace = Namespace {
            metadata: ObjectMeta {
                name: Some(ns.clone()),
                labels: Some(BTreeMap::from([(
                    "signet.sandbox/environment".to_string(),
                    env_id.to_string(),
                )])),
                ..Default::default()
            },
            spec: None,
            status: None,
        };
        Api::<Namespace>::all(self.client.clone())
            .create(&PostParams::default(), &namespace)
            .await?;

        let secret = Secret {
            metadata: ObjectMeta {
                name: Some(SECRET_NAME.to_string()),
                namespace: Some(ns.clone()),
                ..Default::default()
            },
            string_data: Some(BTreeMap::from([
                ("SIGNER_KEY_WIF".to_string(), secrets.signer_wif.clone()),
                ("SIGNER_PUBKEY".to_string(), secrets.signer_pubkey.clone()),
                (
                    "SIGNET_CHALLENGE".to_string(),
                    secrets.signet_challenge.clone(),
                ),
                ("BITCOIN_RPC_USER".to_string(), secrets.rpc_user.clone()),
                (
                    "BITCOIN_RPC_PASSWORD".to_string(),
                    secrets.rpc_password.clone(),
                ),
            ])),
            ..Default::default()
        };
        Api::<Secret>::namespaced(self.client.clone(), &ns)
            .create(&PostParams::default(), &secret)
            .await?;

        self.apply_manifest(&ns, BITCOIND_MANIFEST, images).await?;
        self.apply_manifest(&ns, SIGNER_MANIFEST, images).await?;
        if faucet {
            self.apply_manifest(&ns, FAUCET_MANIFEST, images).await?;
        }
        Ok(())
    }

    pub async fn destroy_environment(&self, env_id: &str) -> Result<(), OrchestrateError> {
        let ns = namespace_for(env_id);
        Api::<Namespace>::all(self.client.clone())
            .delete(&ns, &DeleteParams::default())
            .await?;
        Ok(())
    }

    pub async fn core_ready(&self, env_id: &str) -> Result<bool, OrchestrateError> {
        let ns = namespace_for(env_id);
        let sts: Option<StatefulSet> = Api::<StatefulSet>::namespaced(self.client.clone(), &ns)
            .get("bitcoind")
            .await
            .ok();
        Ok(sts
            .and_then(|s| s.status)
            .and_then(|s| s.ready_replicas)
            .map(|n| n >= 1)
            .unwrap_or(false))
    }

    pub async fn rpc_credentials(
        &self,
        env_id: &str,
    ) -> Result<(String, String), OrchestrateError> {
        let ns = namespace_for(env_id);
        let secret = Api::<Secret>::namespaced(self.client.clone(), &ns)
            .get(SECRET_NAME)
            .await?;
        let data = secret.data.unwrap_or_default();
        let user = data.get("BITCOIN_RPC_USER");
        let pass = data.get("BITCOIN_RPC_PASSWORD");
        match (user, pass) {
            (Some(u), Some(p)) => Ok((
                String::from_utf8_lossy(&u.0).to_string(),
                String::from_utf8_lossy(&p.0).to_string(),
            )),
            _ => Err(OrchestrateError::ManifestKind(SECRET_NAME.to_string())),
        }
    }

    fn split_docs(manifest: &str) -> Vec<String> {
        manifest
            .split("\n---")
            .filter(|d| !d.trim().is_empty())
            .map(|d| format!("{d}\n"))
            .collect()
    }

    async fn apply_manifest(
        &self,
        ns: &str,
        manifest: &str,
        images: &BTreeMap<String, String>,
    ) -> Result<(), OrchestrateError> {
        for doc in Self::split_docs(manifest) {
            let value: Value = serde_yaml::from_str(&doc)?;
            let kind = value["kind"]
                .as_str()
                .ok_or_else(|| OrchestrateError::ManifestKind(doc.chars().take(40).collect()))?;
            match kind {
                "ConfigMap" => {
                    let mut cm: ConfigMap = serde_yaml::from_value(value)?;
                    cm.metadata.namespace = Some(ns.to_string());
                    Api::namespaced(self.client.clone(), ns)
                        .create(&PostParams::default(), &cm)
                        .await?;
                }
                "StatefulSet" => {
                    let mut sts: StatefulSet = serde_yaml::from_value(value)?;
                    sts.metadata.namespace = Some(ns.to_string());
                    if let Some(pod) = sts.spec.as_mut().and_then(|s| s.template.spec.as_mut()) {
                        patch_images(&mut pod.containers, images);
                    }
                    Api::namespaced(self.client.clone(), ns)
                        .create(&PostParams::default(), &sts)
                        .await?;
                }
                "Service" => {
                    let mut svc: Service = serde_yaml::from_value(value)?;
                    svc.metadata.namespace = Some(ns.to_string());
                    Api::namespaced(self.client.clone(), ns)
                        .create(&PostParams::default(), &svc)
                        .await?;
                }
                "Deployment" => {
                    let mut dep: Deployment = serde_yaml::from_value(value)?;
                    dep.metadata.namespace = Some(ns.to_string());
                    if let Some(pod) = dep.spec.as_mut().and_then(|s| s.template.spec.as_mut()) {
                        patch_images(&mut pod.containers, images);
                    }
                    Api::namespaced(self.client.clone(), ns)
                        .create(&PostParams::default(), &dep)
                        .await?;
                }
                other => {
                    return Err(OrchestrateError::ManifestKind(other.to_string()));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct ManifestProbe {
    pub kind: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_parse_into_known_kinds() {
        for manifest in [BITCOIND_MANIFEST, SIGNER_MANIFEST, FAUCET_MANIFEST] {
            for doc in Orchestrator::split_docs(manifest) {
                let probe: ManifestProbe = serde_yaml::from_str(&doc).unwrap();
                assert!(
                    matches!(
                        probe.kind.as_str(),
                        "ConfigMap" | "StatefulSet" | "Service" | "Deployment"
                    ),
                    "unexpected kind {}",
                    probe.kind
                );
            }
        }
    }

    #[test]
    fn split_docs_preserve_trailing_newlines() {
        let docs = Orchestrator::split_docs(BITCOIND_MANIFEST);
        let cm: ConfigMap = serde_yaml::from_str(&docs[0]).unwrap();
        let conf = cm.data.unwrap().get("base.conf").unwrap().clone();
        assert!(
            conf.ends_with('\n'),
            "base.conf lost its trailing newline; appended settings would glue onto the last line"
        );
    }

    #[test]
    fn namespace_derivation_is_prefixed() {
        assert_eq!(namespace_for("9f2a1b"), "env-9f2a1b");
    }

    #[test]
    fn resolve_images_defaults_without_request() {
        let images = resolve_images(&None).unwrap();
        assert_eq!(images.get("bitcoind").unwrap(), "bitcoin/bitcoin:29.4");
        assert_eq!(images.get("signer").unwrap(), "signet-signer:dev");
        assert_eq!(images.get("faucet").unwrap(), "signet-faucet:dev");
    }

    #[test]
    fn resolve_images_merges_user_tags() {
        let tags = Some(BTreeMap::from([(
            "bitcoind".to_string(),
            "28.1".to_string(),
        )]));
        let images = resolve_images(&tags).unwrap();
        assert_eq!(images.get("bitcoind").unwrap(), "bitcoin/bitcoin:28.1");
        assert_eq!(images.get("signer").unwrap(), "signet-signer:dev");
    }

    #[test]
    fn resolve_images_rejects_unknown_component() {
        for component in ["nginx", "faucet"] {
            let tags = Some(BTreeMap::from([(component.to_string(), "1.0".to_string())]));
            assert!(
                matches!(
                    resolve_images(&tags),
                    Err(VersionError::UnknownComponent(_))
                ),
                "{component}"
            );
        }
    }

    #[test]
    fn resolve_images_rejects_bad_tags() {
        for tag in ["", "a b", "$(id)", &"x".repeat(65)] {
            let tags = Some(BTreeMap::from([("bitcoind".to_string(), tag.to_string())]));
            assert!(
                matches!(resolve_images(&tags), Err(VersionError::BadTag(_))),
                "{tag:?}"
            );
        }
    }
}
