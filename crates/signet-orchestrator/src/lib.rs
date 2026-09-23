use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::{Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Secret, Service};
use kube::api::{Api, DeleteParams, ObjectMeta, PostParams};
use kube::core::{ApiResource, DynamicObject};
use kube::{Client, Error};
use serde::Deserialize;
use serde_yaml::Value;

const BITCOIND_MANIFEST: &str = include_str!("../templates/bitcoind.yaml");
const SIGNER_MANIFEST: &str = include_str!("../templates/signer.yaml");
const FAUCET_MANIFEST: &str = include_str!("../templates/faucet.yaml");
const ELECTRS_MANIFEST: &str = include_str!("../templates/electrs.yaml");
const EXPLORER_MANIFEST: &str = include_str!("../templates/explorer.yaml");
const HTTPROUTE_MANIFEST: &str = include_str!("../templates/httproute.yaml");

const SECRET_NAME: &str = "signet-secrets";

const GATEWAY_NAME: &str = "signet-eg";
const GATEWAY_NAMESPACE: &str = "signet-platform";
// Per-env electrs TCP ports (spec §11.2). The local k3d cluster publishes a
// narrow window; hosted clusters publish the full 50002-50502 range.
pub const ELECTRUM_PORT_BASE: i32 = 50002;
pub const ELECTRUM_PORT_MAX: i32 = 50011;

const DEFAULT_BITCOIND_IMAGE: &str = "bitcoin/bitcoin:29.4";
const DEFAULT_SIGNER_IMAGE: &str = "signet-signer:dev";

#[derive(Debug, Clone, Copy, Default)]
pub struct EnvComponents {
    pub indexer: bool,
    pub faucet: bool,
    pub explorer: bool,
}

/// Optional per-env component. Adding a protocol is a new entry here plus a
/// template; the container name must equal `key` so image patching finds it.
pub struct ComponentSpec {
    pub key: &'static str,
    pub manifest: &'static str,
    pub repo: &'static str,
    pub default_tag: &'static str,
    /// Registry keys this component needs; auto-selected when a dependent is.
    pub deps: &'static [&'static str],
    /// Accepts a user image tag in the `versions` map.
    pub versionable: bool,
    pub requested: fn(&EnvComponents) -> bool,
}

impl ComponentSpec {
    pub fn default_image(&self) -> String {
        format!("{}:{}", self.repo, self.default_tag)
    }
}

/// Order is dependency order: a selected component's deps appear before it.
pub const COMPONENTS: &[ComponentSpec] = &[
    ComponentSpec {
        key: "electrs",
        manifest: ELECTRS_MANIFEST,
        repo: "electrs",
        default_tag: "dev",
        deps: &[],
        versionable: true,
        requested: |c| c.indexer,
    },
    ComponentSpec {
        key: "faucet",
        manifest: FAUCET_MANIFEST,
        repo: "signet-faucet",
        default_tag: "dev",
        deps: &[],
        versionable: false,
        requested: |c| c.faucet,
    },
    ComponentSpec {
        key: "explorer",
        manifest: EXPLORER_MANIFEST,
        repo: "mempool/backend",
        default_tag: "v3.3.1",
        deps: &["electrs"],
        versionable: true,
        requested: |c| c.explorer,
    },
];

fn selected_specs(components: &EnvComponents) -> Vec<&'static ComponentSpec> {
    let mut wanted: Vec<&'static str> = COMPONENTS
        .iter()
        .filter(|s| (s.requested)(components))
        .map(|s| s.key)
        .collect();
    let mut i = 0;
    while i < wanted.len() {
        let spec = COMPONENTS
            .iter()
            .find(|s| s.key == wanted[i])
            .expect("selected key must exist in COMPONENTS");
        for dep in spec.deps {
            if !wanted.contains(dep) {
                wanted.push(dep);
            }
        }
        i += 1;
    }
    COMPONENTS
        .iter()
        .filter(|s| wanted.contains(&s.key))
        .collect()
}

fn versionable_repo(key: &str) -> Option<&'static str> {
    match key {
        "bitcoind" => Some("bitcoin/bitcoin"),
        // Versionable per spec §5, but deploys only in P3 (Lightning).
        "lnd" => Some("lightninglabs/lnd"),
        _ => COMPONENTS
            .iter()
            .find(|c| c.key == key && c.versionable)
            .map(|c| c.repo),
    }
}

fn validate_tag(tag: &str) -> Result<(), VersionError> {
    if tag.is_empty()
        || tag.len() > 64
        || !tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(VersionError::BadTag(tag.to_string()));
    }
    Ok(())
}

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
    ]);
    for spec in COMPONENTS {
        images.insert(spec.key.to_string(), spec.default_image());
    }
    let Some(tags) = tags else {
        return Ok(images);
    };
    for (component, tag) in tags {
        validate_tag(tag)?;
        let Some(repo) = versionable_repo(component) else {
            return Err(VersionError::UnknownComponent(component.clone()));
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
    #[error("faucet: {0}")]
    Faucet(String),
    #[error("invalid signet challenge hex: {0}")]
    BadChallenge(String),
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn write_compact_size(out: &mut Vec<u8>, len: usize) {
    if len < 253 {
        out.push(len as u8);
    } else if len <= u16::MAX as usize {
        out.push(253);
        out.extend_from_slice(&(len as u16).to_le_bytes());
    } else {
        out.push(254);
        out.extend_from_slice(&(len as u32).to_le_bytes());
    }
}

fn signet_magic(challenge_hex: &str) -> Result<String, OrchestrateError> {
    use sha2::{Digest, Sha256};
    let challenge = decode_hex(challenge_hex)
        .ok_or_else(|| OrchestrateError::BadChallenge(challenge_hex.into()))?;
    let mut msg = Vec::with_capacity(challenge.len() + 9);
    write_compact_size(&mut msg, challenge.len());
    msg.extend_from_slice(&challenge);
    let first = Sha256::digest(&msg);
    let second = Sha256::digest(first);
    Ok(encode_hex(&second[..4]))
}

fn faucet_fund_request(
    ns: &str,
    address: &str,
    amount_sat: u64,
) -> Result<http::Request<Vec<u8>>, OrchestrateError> {
    let uri = format!("/api/v1/namespaces/{ns}/services/http:faucet:8080/proxy/fund");
    let body = serde_json::json!({ "address": address, "amount_sat": amount_sat });
    http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(body.to_string().into_bytes())
        .map_err(|e| OrchestrateError::Faucet(format!("request build failed: {e}")))
}

fn explorer_hostname(env_host: &str, env_id: &str) -> String {
    format!("env-{env_id}.{env_host}")
}

// `--env-host` accepts `[scheme://]host`; a bare host rides plain HTTP.
fn split_url_base(base: &str) -> (String, String) {
    match base.split_once("://") {
        Some((scheme, host)) => (scheme.to_string(), host.to_string()),
        None => ("http".to_string(), base.to_string()),
    }
}

fn gateway_api(version: &str, kind: &str, plural: &str) -> ApiResource {
    ApiResource {
        group: "gateway.networking.k8s.io".to_string(),
        version: version.to_string(),
        api_version: format!("gateway.networking.k8s.io/{version}"),
        kind: kind.to_string(),
        plural: plural.to_string(),
    }
}

// The bundled CRD serves ReferenceGrant only as v1beta1 (storage+preferred);
// v1 routes would hit an unregistered path and get a plain-text 404.
fn reference_grant_api() -> ApiResource {
    gateway_api("v1beta1", "ReferenceGrant", "referencegrants")
}

fn gateway_object() -> DynamicObject {
    let mut gateway = DynamicObject::new(GATEWAY_NAME, &gateway_api("v1", "Gateway", "gateways"));
    gateway.metadata.namespace = Some(GATEWAY_NAMESPACE.to_string());
    gateway.data = serde_json::json!({
        "spec": {
            "gatewayClassName": "signet-eg",
            "listeners": [{
                "name": "http",
                "protocol": "HTTP",
                "port": 80,
                "allowedRoutes": { "namespaces": { "from": "All" } },
            }],
        }
    });
    gateway
}

fn route_grant_object(env_ns: &str) -> DynamicObject {
    let mut grant = DynamicObject::new(env_ns, &reference_grant_api());
    grant.metadata.namespace = Some(GATEWAY_NAMESPACE.to_string());
    grant.data = serde_json::json!({
        "spec": {
            "from": [{
                "group": "gateway.networking.k8s.io",
                "kind": "HTTPRoute",
                "namespace": env_ns,
            }],
            "to": [{ "group": "gateway.networking.k8s.io", "kind": "Gateway", "name": GATEWAY_NAME }],
        }
    });
    grant
}

fn route_document(hostname: &str) -> Result<DynamicObject, OrchestrateError> {
    let mut value = Orchestrator::parse_docs(HTTPROUTE_MANIFEST)?
        .into_iter()
        .next()
        .ok_or_else(|| OrchestrateError::ManifestKind("empty httproute template".into()))?;
    value["spec"]["hostnames"] =
        serde_yaml::Value::Sequence(vec![serde_yaml::Value::String(hostname.to_string())]);
    Ok(serde_yaml::from_value(value)?)
}

pub struct Orchestrator {
    client: Client,
    env_host: String,
    url_scheme: String,
}

impl Orchestrator {
    pub async fn connect(env_host: &str) -> Result<Self, OrchestrateError> {
        let (url_scheme, host) = split_url_base(env_host);
        Ok(Self {
            client: Client::try_default().await?,
            env_host: host,
            url_scheme,
        })
    }

    pub fn explorer_url(&self, env_id: &str) -> String {
        format!(
            "{}://{}",
            self.url_scheme,
            explorer_hostname(&self.env_host, env_id)
        )
    }

    pub async fn create_environment(
        &self,
        env_id: &str,
        secrets: &EnvSecrets,
        images: &BTreeMap<String, String>,
        components: EnvComponents,
        electrum_port: Option<u16>,
    ) -> Result<(), OrchestrateError> {
        let ns = namespace_for(env_id);

        self.ensure_gateway().await?;

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
                (
                    "BITCOIN_RPC_COOKIE".to_string(),
                    format!("{}:{}", secrets.rpc_user, secrets.rpc_password),
                ),
                (
                    "SIGNET_MAGIC".to_string(),
                    signet_magic(&secrets.signet_challenge)?,
                ),
            ])),
            ..Default::default()
        };
        Api::<Secret>::namespaced(self.client.clone(), &ns)
            .create(&PostParams::default(), &secret)
            .await?;

        self.apply_manifest(&ns, BITCOIND_MANIFEST, images).await?;
        self.apply_manifest(&ns, SIGNER_MANIFEST, images).await?;
        for spec in selected_specs(&components) {
            self.apply_manifest(&ns, spec.manifest, images).await?;
        }
        if components.explorer {
            self.apply_explorer_route(&ns, env_id).await?;
        }
        if let Some(port) = electrum_port {
            self.apply_electrs_lb(&ns, port).await?;
        }
        if components.explorer || electrum_port.is_some() {
            self.ensure_route_grant(&ns).await?;
        }
        Ok(())
    }

    pub async fn destroy_environment(&self, env_id: &str) -> Result<(), OrchestrateError> {
        let ns = namespace_for(env_id);
        Api::<Namespace>::all(self.client.clone())
            .delete(&ns, &DeleteParams::default())
            .await?;
        // The HTTPRoute dies with the namespace cascade. The ReferenceGrant
        // lives in the Gateway namespace and is best-effort: a grant naming a
        // deleted namespace authorizes nothing, so a failed cleanup must not
        // mask the destroy.
        let grants = Api::<DynamicObject>::namespaced_with(
            self.client.clone(),
            GATEWAY_NAMESPACE,
            &reference_grant_api(),
        );
        if let Err(e) = grants.delete(&ns, &DeleteParams::default()).await
            && !matches!(&e, Error::Api(status) if status.code == 404)
        {
            tracing::warn!(error = %e, env_ns = %ns, "ReferenceGrant cleanup failed");
        }
        Ok(())
    }

    async fn ensure_gateway(&self) -> Result<(), OrchestrateError> {
        let namespaces = Api::<Namespace>::all(self.client.clone());
        if namespaces.get_opt(GATEWAY_NAMESPACE).await?.is_none() {
            namespaces
                .create(
                    &PostParams::default(),
                    &Namespace {
                        metadata: ObjectMeta {
                            name: Some(GATEWAY_NAMESPACE.to_string()),
                            ..Default::default()
                        },
                        spec: None,
                        status: None,
                    },
                )
                .await?;
        }
        let gateways = Api::<DynamicObject>::namespaced_with(
            self.client.clone(),
            GATEWAY_NAMESPACE,
            &gateway_api("v1", "Gateway", "gateways"),
        );
        if gateways.get_opt(GATEWAY_NAME).await?.is_none() {
            gateways
                .create(&PostParams::default(), &gateway_object())
                .await?;
        }
        Ok(())
    }

    async fn apply_explorer_route(&self, ns: &str, env_id: &str) -> Result<(), OrchestrateError> {
        let hostname = explorer_hostname(&self.env_host, env_id);
        let mut route = route_document(&hostname)?;
        route.metadata.namespace = Some(ns.to_string());
        Api::<DynamicObject>::namespaced_with(
            self.client.clone(),
            ns,
            &gateway_api("v1", "HTTPRoute", "httproutes"),
        )
        .create(&PostParams::default(), &route)
        .await?;
        Ok(())
    }

    // Electrum exposure for one env: a LoadBalancer Service whose port equals
    // the env's allocated electrum port (klipper binds it on the node; the
    // local k3d cluster publishes the same host ports).
    async fn apply_electrs_lb(&self, ns: &str, port: u16) -> Result<(), OrchestrateError> {
        let svc = Service {
            metadata: ObjectMeta {
                name: Some("electrs-lb".to_string()),
                namespace: Some(ns.to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::ServiceSpec {
                type_: Some("LoadBalancer".to_string()),
                selector: Some(BTreeMap::from([("app".to_string(), "electrs".to_string())])),
                ports: Some(vec![k8s_openapi::api::core::v1::ServicePort {
                    port: port as i32,
                    target_port: Some(
                        k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(60401),
                    ),
                    protocol: Some("TCP".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            status: None,
        };
        Api::<Service>::namespaced(self.client.clone(), ns)
            .create(&PostParams::default(), &svc)
            .await?;
        Ok(())
    }

    async fn ensure_route_grant(&self, env_ns: &str) -> Result<(), OrchestrateError> {
        let grants = Api::<DynamicObject>::namespaced_with(
            self.client.clone(),
            GATEWAY_NAMESPACE,
            &reference_grant_api(),
        );
        if grants.get_opt(env_ns).await?.is_none() {
            grants
                .create(&PostParams::default(), &route_grant_object(env_ns))
                .await?;
        }
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

    pub async fn faucet_fund(
        &self,
        env_id: &str,
        address: &str,
        amount_sat: u64,
    ) -> Result<String, OrchestrateError> {
        let ns = namespace_for(env_id);
        let request = faucet_fund_request(&ns, address, amount_sat)?;
        let text = self.client.request_text(request).await?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| OrchestrateError::Faucet(format!("bad faucet response: {e}")))?;
        let txid = value
            .get("txid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| OrchestrateError::Faucet("faucet response missing txid".to_string()))?;
        Ok(txid.to_string())
    }

    fn parse_docs(manifest: &str) -> Result<Vec<Value>, OrchestrateError> {
        serde_yaml::Deserializer::from_str(manifest)
            .map(Value::deserialize)
            .collect::<Result<Vec<_>, _>>()
            .map_err(OrchestrateError::Manifest)
    }

    async fn apply_manifest(
        &self,
        ns: &str,
        manifest: &str,
        images: &BTreeMap<String, String>,
    ) -> Result<(), OrchestrateError> {
        for value in Self::parse_docs(manifest)? {
            let kind = value["kind"].as_str().ok_or_else(|| {
                let preview = serde_yaml::to_string(&value).unwrap_or_default();
                OrchestrateError::ManifestKind(preview.chars().take(40).collect())
            })?;
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
        for manifest in [
            BITCOIND_MANIFEST,
            SIGNER_MANIFEST,
            FAUCET_MANIFEST,
            ELECTRS_MANIFEST,
            EXPLORER_MANIFEST,
            HTTPROUTE_MANIFEST,
        ] {
            for value in Orchestrator::parse_docs(manifest).unwrap() {
                let probe: ManifestProbe = serde_yaml::from_value(value).unwrap();
                assert!(
                    matches!(
                        probe.kind.as_str(),
                        "ConfigMap"
                            | "StatefulSet"
                            | "Service"
                            | "Deployment"
                            | "HTTPRoute"
                            | "TCPRoute"
                    ),
                    "unexpected kind {}",
                    probe.kind
                );
            }
        }
    }

    #[test]
    fn parsed_configmaps_keep_trailing_newlines() {
        let docs = Orchestrator::parse_docs(BITCOIND_MANIFEST).unwrap();
        let cm: ConfigMap = serde_yaml::from_value(docs[0].clone()).unwrap();
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
    fn faucet_fund_request_targets_service_proxy() {
        let request = faucet_fund_request(
            "env-abc123",
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx",
            5000,
        )
        .unwrap();
        assert_eq!(request.method(), "POST");
        assert_eq!(
            request.uri(),
            "/api/v1/namespaces/env-abc123/services/http:faucet:8080/proxy/fund"
        );
        let body: serde_json::Value = serde_json::from_slice(request.body()).unwrap();
        assert_eq!(
            body["address"],
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx"
        );
        assert_eq!(body["amount_sat"], 5000);
    }

    #[test]
    fn resolve_images_defaults_without_request() {
        let images = resolve_images(&None).unwrap();
        for (key, image) in [
            ("bitcoind", "bitcoin/bitcoin:29.4"),
            ("signer", "signet-signer:dev"),
            ("faucet", "signet-faucet:dev"),
            ("electrs", "electrs:dev"),
            ("explorer", "mempool/backend:v3.3.1"),
        ] {
            assert_eq!(images.get(key).unwrap(), image, "{key}");
        }
    }

    #[test]
    fn selected_specs_resolve_dependency_order() {
        let none = EnvComponents::default();
        assert!(selected_specs(&none).is_empty());

        let explorer_only = EnvComponents {
            explorer: true,
            ..Default::default()
        };
        let keys: Vec<_> = selected_specs(&explorer_only)
            .iter()
            .map(|s| s.key)
            .collect();
        assert_eq!(keys, vec!["electrs", "explorer"]);

        let faucet_only = EnvComponents {
            faucet: true,
            ..Default::default()
        };
        let keys: Vec<_> = selected_specs(&faucet_only).iter().map(|s| s.key).collect();
        assert_eq!(keys, vec!["faucet"]);
    }

    #[test]
    fn explorer_deployment_wires_bitcoind_and_electrs() {
        let docs = Orchestrator::parse_docs(EXPLORER_MANIFEST).unwrap();
        let deployment: Deployment = serde_yaml::from_value(docs[0].clone()).unwrap();
        let pod = deployment.spec.unwrap().template.spec.unwrap();
        let container = &pod.containers[0];
        assert_eq!(container.name, "explorer");
        let env = container.env.as_ref().unwrap();
        for (name, value) in [
            ("MEMPOOL_NETWORK", "signet"),
            ("MEMPOOL_BACKEND", "electrum"),
            ("CORE_RPC_HOST", "bitcoind"),
            ("ELECTRUM_HOST", "electrs"),
            ("ELECTRUM_PORT", "60401"),
            ("DATABASE_ENABLED", "false"),
            ("STATISTICS_ENABLED", "false"),
        ] {
            assert!(
                env.iter()
                    .any(|e| e.name == name && e.value.as_deref() == Some(value)),
                "{name}"
            );
        }
        for (name, key) in [
            ("CORE_RPC_USERNAME", "BITCOIN_RPC_USER"),
            ("CORE_RPC_PASSWORD", "BITCOIN_RPC_PASSWORD"),
        ] {
            let secret_ref = env
                .iter()
                .find(|e| e.name == name)
                .and_then(|e| e.value_from.as_ref())
                .and_then(|v| v.secret_key_ref.as_ref())
                .unwrap_or_else(|| panic!("{name} must come from the secret"));
            assert_eq!(secret_ref.name, SECRET_NAME);
            assert_eq!(secret_ref.key, key);
        }
        let web = &pod.containers[1];
        assert_eq!(web.name, "explorer-web");
        let web_env = web.env.as_ref().unwrap();
        assert!(
            web_env.iter().any(|e| e.name == "BACKEND_MAINNET_HTTP_HOST"
                && e.value.as_deref() == Some("localhost"))
        );

        let probe = container
            .readiness_probe
            .as_ref()
            .unwrap()
            .http_get
            .as_ref()
            .unwrap();
        assert_eq!(
            probe.port,
            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String("api".into())
        );
    }

    #[test]
    fn electrs_container_reads_cookie_from_platform_secret() {
        let docs = Orchestrator::parse_docs(ELECTRS_MANIFEST).unwrap();
        let sts: StatefulSet = serde_yaml::from_value(docs[0].clone()).unwrap();
        let pod = sts.spec.unwrap().template.spec.unwrap();
        let container = &pod.containers[0];
        assert_eq!(container.name, "electrs");
        let env = container.env.as_ref().unwrap();
        for name in [
            "ELECTRS_NETWORK",
            "ELECTRS_DB_DIR",
            "ELECTRS_DAEMON_RPC_ADDR",
            "ELECTRS_ELECTRUM_RPC_ADDR",
            "ELECTRS_MAGIC",
        ] {
            assert!(env.iter().any(|e| e.name == name), "missing env var {name}");
        }
        let magic = env
            .iter()
            .find(|e| e.name == "ELECTRS_MAGIC")
            .and_then(|e| e.value_from.as_ref())
            .and_then(|v| v.secret_key_ref.as_ref())
            .unwrap();
        assert_eq!(magic.name, SECRET_NAME);
        assert_eq!(magic.key, "SIGNET_MAGIC");
        let cookie_volume = pod
            .volumes
            .as_ref()
            .and_then(|v| {
                v.iter().find(|v| {
                    v.secret
                        .as_ref()
                        .and_then(|s| s.secret_name.as_deref())
                        .is_some_and(|name| name == SECRET_NAME)
                })
            })
            .expect("cookie volume mounts signet-secrets");
        let mount = container
            .volume_mounts
            .as_ref()
            .unwrap()
            .iter()
            .find(|m| m.name == cookie_volume.name)
            .expect("cookie volume is mounted");
        assert_eq!(mount.mount_path, "/etc/electrs");
        let args = container.command.as_ref().unwrap();
        assert!(
            args.iter()
                .any(|a| a.contains("--cookie-file=/etc/electrs/cookie")),
            "electrs must authenticate with the platform cookie file"
        );
    }

    #[test]
    fn signet_magic_matches_bitcoind_derivation() {
        let challenge =
            "51210266c545524adda007692aae987b5c192ceabf17b2bf52f0e38d3ad76a8c76c5ad51ae";
        assert_eq!(
            signet_magic(challenge).unwrap(),
            "7b523e9e",
            "magic must equal Core's `Signet derived magic (message start)` for the challenge"
        );
        assert!(matches!(
            signet_magic("zz"),
            Err(OrchestrateError::BadChallenge(_))
        ));
    }

    #[test]
    fn compact_size_encoding_matches_core() {
        for (len, expected) in [
            (0u64, vec![0u8]),
            (252, vec![252]),
            (253, vec![253, 253, 0]),
            (65535, vec![253, 255, 255]),
            (65536, vec![254, 0, 0, 1, 0]),
        ] {
            let mut out = Vec::new();
            write_compact_size(&mut out, len as usize);
            assert_eq!(out, expected, "len {len}");
        }
    }

    #[test]
    fn patch_images_renames_electrs_container_image() {
        let mut containers = vec![k8s_openapi::api::core::v1::Container {
            name: "electrs".into(),
            ..Default::default()
        }];
        let images = resolve_images(&Some(BTreeMap::from([(
            "electrs".to_string(),
            "0.10.9".to_string(),
        )])))
        .unwrap();
        patch_images(&mut containers, &images);
        assert_eq!(containers[0].image.as_deref(), Some("electrs:0.10.9"));
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
    fn resolve_images_versions_explorer_tag() {
        let tags = Some(BTreeMap::from([
            ("explorer".to_string(), "v3.3.0".to_string()),
            ("lnd".to_string(), "0.18.5-beta".to_string()),
        ]));
        let images = resolve_images(&tags).unwrap();
        assert_eq!(images.get("explorer").unwrap(), "mempool/backend:v3.3.0");
        assert_eq!(images.get("lnd").unwrap(), "lightninglabs/lnd:0.18.5-beta");
    }

    #[test]
    fn resolve_images_rejects_unknown_component() {
        for component in ["nginx", "faucet", "signer"] {
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

    #[test]
    fn explorer_hostname_routes_the_env_subdomain() {
        assert_eq!(
            explorer_hostname("localhost", "9f2a1b"),
            "env-9f2a1b.localhost"
        );
        assert_eq!(
            explorer_hostname("sandbox.signet.dev", "9f2a1b"),
            "env-9f2a1b.sandbox.signet.dev"
        );
    }

    #[test]
    fn split_url_base_schemes_the_env_host() {
        assert_eq!(
            split_url_base("localhost"),
            ("http".to_string(), "localhost".to_string())
        );
        assert_eq!(
            split_url_base("https://sandbox.signet.dev"),
            ("https".to_string(), "sandbox.signet.dev".to_string())
        );
    }

    #[test]
    fn gateway_object_serves_http_for_all_namespaces() {
        let gateway = serde_json::to_value(gateway_object()).unwrap();
        assert_eq!(gateway["metadata"]["name"], GATEWAY_NAME);
        assert_eq!(gateway["spec"]["gatewayClassName"], "signet-eg");
        let listener = &gateway["spec"]["listeners"][0];
        assert_eq!(listener["name"], "http");
        assert_eq!(listener["allowedRoutes"]["namespaces"]["from"], "All");
    }

    #[test]
    fn route_grant_bridges_the_env_namespace_to_the_gateway() {
        let grant = serde_json::to_value(route_grant_object("env-abc123")).unwrap();
        assert_eq!(grant["metadata"]["namespace"], GATEWAY_NAMESPACE);
        assert_eq!(grant["metadata"]["name"], "env-abc123");
        assert_eq!(grant["spec"]["from"][0]["kind"], "HTTPRoute");
        assert_eq!(grant["spec"]["from"][0]["namespace"], "env-abc123");
        assert_eq!(grant["spec"]["to"][0]["kind"], "Gateway");
        assert_eq!(grant["spec"]["to"][0]["name"], GATEWAY_NAME);
    }

    #[test]
    fn route_document_patches_hostname_and_targets_explorer() {
        let route = route_document("env-abc123.localhost").unwrap();
        assert_eq!(route.types.as_ref().unwrap().kind.as_str(), "HTTPRoute");
        assert_eq!(route.data["spec"]["hostnames"][0], "env-abc123.localhost");
        assert_eq!(route.data["spec"]["parentRefs"][0]["name"], GATEWAY_NAME);
        assert_eq!(
            route.data["spec"]["parentRefs"][0]["namespace"],
            GATEWAY_NAMESPACE
        );
        assert_eq!(
            route.data["spec"]["rules"][0]["backendRefs"][0]["name"],
            "explorer"
        );
    }
}
