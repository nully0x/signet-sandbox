use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use k8s_openapi::api::apps::v1::{Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Pod, Secret, Service};
use kube::api::{Api, AttachParams, ListParams, ObjectMeta, Patch, PatchParams, PostParams};
use kube::core::{ApiResource, DynamicObject};
use kube::{Client, Error};
use serde::Deserialize;
use serde_json::json;
use serde_yaml::Value;
use signet_core::env::LightningInstance;
use signet_core::versions::VersionCatalog;

mod reaper;
mod tls;

const BITCOIND_MANIFEST: &str = include_str!("../templates/bitcoind.yaml");
const SIGNER_MANIFEST: &str = include_str!("../templates/signer.yaml");
const FAUCET_MANIFEST: &str = include_str!("../templates/faucet.yaml");
const LND_MANIFEST: &str = include_str!("../templates/lnd.yaml");
const LND_MACAROON_PATH: &str = "/root/.lnd/data/chain/bitcoin/signet/admin.macaroon";
/// lncli's default `~/.lnd/tls.cert` no longer exists; the provisioned
/// leaf cert lives under /root/.lnd/tls and must be passed explicitly.
const LND_TLS_CERT_FLAG: &str = "--tlscertpath=/root/.lnd/tls/tls.crt";
const LND_WALLET_PASSWORD_KEY: &str = "LND_WALLET_PASSWORD";
const ELECTRS_MANIFEST: &str = include_str!("../templates/electrs.yaml");
const EXPLORER_MANIFEST: &str = include_str!("../templates/explorer.yaml");
const HTTPROUTE_MANIFEST: &str = include_str!("../templates/httproute.yaml");

const SECRET_NAME: &str = "signet-secrets";

/// Marks a namespace as a signet environment namespace.
pub const ENV_LABEL: &str = "signet.sandbox/environment";

/// RFC 3339 timestamp; past-due namespaces are reaped.
pub const EXPIRES_AT_ANNOTATION: &str = "signet.sandbox/expires-at";

const GATEWAY_NAME: &str = "signet-eg";
const GATEWAY_NAMESPACE: &str = "signet-platform";
// Per-env electrs TCP ports. The pool sits BELOW the ephemeral port
// range (32768-60999): inside it, any host service can permanently
// occupy a port and break every cluster port mapping. Keep the window
// in sync with the k3d port maps (justfile cluster-up, signet-cli
// local.rs) — the k3d load balancer fails on multi-thousand-port maps.
pub const ELECTRUM_PORT_BASE: i32 = 20000;
pub const ELECTRUM_PORT_MAX: i32 = 20599;

// Per-instance lightning TCP ports: REST on the port, gRPC on port + 1,
// p2p on port + 2 (one port triple per LND instance, allocated from the
// same window, above the electrs range). Mapped by k3d at cluster create
// like the electrs window.
pub const LIGHTNING_PORT_BASE: i32 = 21000;
pub const LIGHTNING_PORT_MAX: i32 = 21199;

const DEFAULT_SIGNER_IMAGE: &str = "signet-signer:dev";

#[derive(Debug, Clone, Default)]
pub struct EnvComponents {
    pub indexer: bool,
    pub faucet: bool,
    pub explorer: bool,
    pub lightning: Vec<LightningInstance>,
}

/// Optional per-env component. Adding a protocol is a new entry here plus a
/// template; `containers` maps every pod container to its platform-owned repo
/// so image patching finds it.
/// Versionable components: (key, repo, registry prefix). Registries:
/// upstream images live on docker.io (empty prefix); self-published ones
/// on the platform registry.
pub const VERSIONED_COMPONENTS: &[(&str, &str, &str)] = &[
    ("bitcoind", "bitcoin/bitcoin", "docker.io/"),
    ("electrs", "electrs", "ghcr.io/nully0x/"),
    ("lnd", "lightninglabs/lnd", "docker.io/"),
    ("explorer", "mempool/backend", "docker.io/"),
];

pub struct ComponentSpec {
    pub key: &'static str,
    pub manifest: &'static str,
    pub repo: &'static str,
    pub default_tag: &'static str,
    /// Registry keys this component needs; auto-selected when a dependent is.
    pub deps: &'static [&'static str],
    /// Accepts a user image tag in the `versions` map.
    pub versionable: bool,
    /// (container name, repo) pairs a resolved tag applies to. Multi-container
    /// pods pin all containers to the same tag (mempool backend + frontend).
    pub containers: &'static [(&'static str, &'static str)],
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
        containers: &[("electrs", "electrs")],
        requested: |c| c.indexer,
    },
    ComponentSpec {
        key: "faucet",
        manifest: FAUCET_MANIFEST,
        repo: "signet-faucet",
        default_tag: "dev",
        deps: &[],
        versionable: false,
        containers: &[("faucet", "signet-faucet")],
        requested: |c| c.faucet,
    },
    ComponentSpec {
        key: "explorer",
        manifest: EXPLORER_MANIFEST,
        repo: "mempool/backend",
        default_tag: "v3.3.1",
        deps: &["electrs"],
        versionable: true,
        containers: &[
            ("explorer", "mempool/backend"),
            ("explorer-web", "mempool/frontend"),
        ],
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
    #[error("{0}")]
    Incompatible(String),
}

/// Resolve the final image map for an environment: platform components
/// keep their local dev tags; versionable components resolve to
/// registry-qualified refs — the user's pin, or the catalog default.
/// Pinned versions must exist in the catalog and satisfy its
/// compatibility constraints.
pub fn resolve_images(
    catalog: &VersionCatalog,
    tags: &Option<BTreeMap<String, String>>,
) -> Result<BTreeMap<String, String>, VersionError> {
    if let Some(tags) = tags {
        for tag in tags.values() {
            validate_tag(tag)?;
        }
    }
    if let Err(message) = catalog.validate_selection(tags.as_ref().unwrap_or(&BTreeMap::new())) {
        return Err(VersionError::Incompatible(message));
    }
    let mut images = BTreeMap::from([
        ("signer".to_string(), DEFAULT_SIGNER_IMAGE.to_string()),
        ("faucet".to_string(), "signet-faucet:dev".to_string()),
    ]);
    for (component, repo, registry) in VERSIONED_COMPONENTS {
        let tag = tags
            .as_ref()
            .and_then(|tags| tags.get(*component))
            .map(String::as_str)
            .or_else(|| catalog.default_version(component))
            .ok_or_else(|| VersionError::UnknownComponent(component.to_string()))?;
        validate_tag(tag)?;
        images.insert(component.to_string(), format!("{registry}{repo}:{tag}"));
    }
    // The explorer pod pins backend and frontend to the same tag.
    if let Some(backend) = images.get("explorer").cloned() {
        let frontend = backend.replacen("mempool/backend", "mempool/frontend", 1);
        images.insert("explorer-web".to_string(), frontend);
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

/// Component-keyed resolved images → container-keyed patch set. Multi-
/// container pods take the same tag on each container's own repo
/// (mempool backend and frontend ship as a matched pair).
fn container_images(images: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut patched = BTreeMap::new();
    for (key, image) in images {
        match COMPONENTS.iter().find(|s| s.key == key) {
            Some(spec) => {
                // Multi-container pods share the tag; swap the repo in the
                // resolved (registry-qualified) ref.
                let first_repo = spec.containers[0].1;
                for (container, repo) in spec.containers {
                    let reference = image.replacen(first_repo, repo, 1);
                    patched.insert(container.to_string(), reference);
                }
            }
            // bitcoind, signer: the container name equals the key.
            None => {
                patched.insert(key.clone(), image.clone());
            }
        }
    }
    patched
}

pub fn namespace_for(env_id: &str) -> String {
    format!("env-{env_id}")
}

/// The lnd instance runs as a StatefulSet, so its pod name is stable.
/// All exec traffic (wallet init, file reads) targets this name.
fn lnd_pod_name(instance: &str) -> String {
    format!("lnd-{instance}-0")
}

fn namespace_object(env_id: &str, expires_at: Option<DateTime<Utc>>) -> Namespace {
    let mut metadata = ObjectMeta {
        name: Some(namespace_for(env_id)),
        labels: Some(BTreeMap::from([(
            ENV_LABEL.to_string(),
            env_id.to_string(),
        )])),
        ..Default::default()
    };
    if let Some(ts) = expires_at {
        metadata.annotations = Some(BTreeMap::from([(
            EXPIRES_AT_ANNOTATION.to_string(),
            ts.to_rfc3339(),
        )]));
    }
    Namespace {
        metadata,
        spec: None,
        status: None,
    }
}

#[derive(Debug, Clone)]
pub struct EnvSecrets {
    pub signer_wif: String,
    pub signer_pubkey: String,
    pub signet_challenge: String,
    pub rpc_user: String,
    pub rpc_password: String,
    pub lnd_wallet_password: String,
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
    #[error("{0}")]
    Incompatible(String),
    #[error("pod exec failed: {0}")]
    Exec(String),
    #[error("lnd tls generation: {0}")]
    Tls(#[from] rcgen::Error),
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

/// lnd names its hex identity key `identity_pubkey` in getinfo output;
/// CLN calls the same value `id`. The bundle field is `node_id`.
fn parse_node_id(out: &[u8]) -> Result<String, OrchestrateError> {
    let json: serde_json::Value = serde_json::from_slice(out)
        .map_err(|e| OrchestrateError::Exec(format!("getinfo output is not JSON: {e}")))?;
    json.get("identity_pubkey")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| OrchestrateError::Exec("getinfo has no identity_pubkey".into()))
}

/// Base lncli invocation: signet network plus the provisioned cert path
/// (lncli's default `~/.lnd/tls.cert` no longer exists).
fn lncli_base() -> [&'static str; 3] {
    ["lncli", "--network=signet", LND_TLS_CERT_FLAG]
}

fn parse_peer_keys(out: &[u8]) -> Result<Vec<String>, OrchestrateError> {
    let json: serde_json::Value = serde_json::from_slice(out)
        .map_err(|e| OrchestrateError::Exec(format!("listpeers output is not JSON: {e}")))?;
    Ok(json
        .get("peers")
        .and_then(|v| v.as_array())
        .map(|peers| {
            peers
                .iter()
                .filter_map(|p| p.get("pub_key").and_then(|k| k.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default())
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

// One instance's manifest with its name substituted; the public host
// placeholder is filled in by the caller at apply time.
fn lnd_manifest(instance: &LightningInstance) -> String {
    LND_MANIFEST.replace("LND_NAME", &instance.name)
}

// `--env-host` accepts `[scheme://]host`; a bare host rides plain HTTP.
fn split_url_base(base: &str) -> (String, String) {
    match base.split_once("://") {
        Some((scheme, host)) => (scheme.to_string(), host.to_string()),
        None => ("http".to_string(), base.to_string()),
    }
}

// The lightning public host: the non-empty override wins, else the env
// host's host part without port. An empty override keeps the default
// single-knob shape (blank env vars are harmless).
fn resolve_ln_host(env_host: &str, ln_host: Option<&str>) -> String {
    match ln_host.map(str::trim).filter(|h| !h.is_empty()) {
        Some(h) => split_url_base(h)
            .1
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string(),
        None => split_url_base(env_host)
            .1
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string(),
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

/// Everything one environment's provisioning needs, in one place.
pub struct ProvisionParams<'a> {
    pub env_id: &'a str,
    pub secrets: &'a EnvSecrets,
    pub images: &'a BTreeMap<String, String>,
    pub components: EnvComponents,
    pub electrum_port: Option<u16>,
    pub lightning: &'a [(LightningInstance, i32)],
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct Orchestrator {
    client: Client,
    env_host: String,
    /// Host part of env_host (no scheme, no port): the name lightning
    /// endpoint URLs use, and the cert SAN when it is an IP or domain.
    /// An explicit `--ln-host` override keeps the explorer's
    /// `env-<id>.<env_host>` wildcard intact while lightning clients
    /// dial a LAN IP or a different domain.
    ln_host: String,
    url_scheme: String,
}

impl Orchestrator {
    pub async fn connect(env_host: &str, ln_host: Option<&str>) -> Result<Self, OrchestrateError> {
        let (url_scheme, host) = split_url_base(env_host);
        let ln_host = resolve_ln_host(env_host, ln_host);
        Ok(Self {
            client: Client::try_default().await?,
            env_host: host,
            ln_host,
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
        params: &ProvisionParams<'_>,
    ) -> Result<(), OrchestrateError> {
        let env_id = params.env_id;
        let ns = namespace_for(env_id);

        self.ensure_gateway().await?;

        let namespace = namespace_object(env_id, params.expires_at);
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
                (
                    "SIGNER_KEY_WIF".to_string(),
                    params.secrets.signer_wif.clone(),
                ),
                (
                    "SIGNER_PUBKEY".to_string(),
                    params.secrets.signer_pubkey.clone(),
                ),
                (
                    "SIGNET_CHALLENGE".to_string(),
                    params.secrets.signet_challenge.clone(),
                ),
                (
                    "BITCOIN_RPC_USER".to_string(),
                    params.secrets.rpc_user.clone(),
                ),
                (
                    "BITCOIN_RPC_PASSWORD".to_string(),
                    params.secrets.rpc_password.clone(),
                ),
                (
                    "BITCOIN_RPC_COOKIE".to_string(),
                    format!(
                        "{}:{}",
                        params.secrets.rpc_user, params.secrets.rpc_password
                    ),
                ),
                (
                    "LND_WALLET_PASSWORD".to_string(),
                    params.secrets.lnd_wallet_password.clone(),
                ),
                (
                    "SIGNET_MAGIC".to_string(),
                    signet_magic(&params.secrets.signet_challenge)?,
                ),
            ])),
            ..Default::default()
        };
        Api::<Secret>::namespaced(self.client.clone(), &ns)
            .create(&PostParams::default(), &secret)
            .await?;

        self.apply_manifest(&ns, BITCOIND_MANIFEST, params.images)
            .await?;
        self.apply_manifest(&ns, SIGNER_MANIFEST, params.images)
            .await?;
        for spec in selected_specs(&params.components) {
            self.apply_manifest(&ns, spec.manifest, params.images)
                .await?;
        }
        for (instance, rest_port) in params.lightning {
            self.apply_lnd(&ns, instance, params.images, *rest_port)
                .await?;
        }
        // lnd self-bootstraps: with --noseedbackup it creates and
        // unlocks its wallet on boot. Provisioning never waits for it —
        // connection bundles carry an instance only once its wallet
        // answers, and clients poll for that when they need lightning.
        if params.components.explorer {
            self.apply_explorer_route(&ns, env_id).await?;
        }
        if let Some(port) = params.electrum_port {
            self.apply_electrs_lb(&ns, port).await?;
        }
        if params.components.explorer || params.electrum_port.is_some() {
            self.ensure_route_grant(&ns).await?;
        }
        Ok(())
    }

    /// Apply one Lightning instance's TLS material, manifest, and
    /// LoadBalancer service. The template's LND_NAME placeholder carries
    /// the instance name and the container image comes from the resolved
    /// versions map. The TLS secret is created once: a re-apply keeps the
    /// original certs, so client-pinned roots stay valid.
    async fn apply_lnd(
        &self,
        ns: &str,
        instance: &LightningInstance,
        images: &BTreeMap<String, String>,
        rest_port: i32,
    ) -> Result<(), OrchestrateError> {
        let instance_tls = tls::generate_instance_tls(&instance.name, ns, &self.ln_host)?;
        self.apply_lnd_tls_secret(ns, &instance.name, &instance_tls)
            .await?;
        let manifest = lnd_manifest(instance);
        self.apply_manifest(ns, &manifest, images).await?;
        self.apply_lnd_lb(ns, instance, rest_port).await
    }

    /// Create the instance's TLS secret (`ca.crt`, `tls.crt` chain,
    /// `tls.key`). The pod mounts it read-only; LND serves the leaf+CA
    /// chain from it, which strict clients verify against the pinned CA.
    async fn apply_lnd_tls_secret(
        &self,
        ns: &str,
        instance: &str,
        instance_tls: &tls::InstanceTls,
    ) -> Result<(), OrchestrateError> {
        let secret = Secret {
            metadata: ObjectMeta {
                name: Some(format!("lnd-{instance}-tls")),
                namespace: Some(ns.to_string()),
                ..Default::default()
            },
            string_data: Some(BTreeMap::from([
                ("ca.crt".to_string(), instance_tls.ca_pem.clone()),
                ("tls.crt".to_string(), instance_tls.chain_pem.clone()),
                ("tls.key".to_string(), instance_tls.key_pem.clone()),
            ])),
            ..Default::default()
        };
        let secrets: Api<Secret> = Api::namespaced(self.client.clone(), ns);
        match secrets.create(&PostParams::default(), &secret).await {
            Ok(_) => Ok(()),
            Err(Error::Api(api_error)) if api_error.code == 409 => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// REST + gRPC exposure for one instance: a LoadBalancer service
    /// (klipper binds it on the node; the local k3d cluster publishes the
    /// same host ports) whose two ports land on the lnd listeners.
    async fn apply_lnd_lb(
        &self,
        ns: &str,
        instance: &LightningInstance,
        rest_port: i32,
    ) -> Result<(), OrchestrateError> {
        let svc = Service {
            metadata: ObjectMeta {
                name: Some(format!("lnd-lb-{}", instance.name)),
                namespace: Some(ns.to_string()),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::core::v1::ServiceSpec {
                type_: Some("LoadBalancer".to_string()),
                selector: Some(BTreeMap::from([(
                    "app".to_string(),
                    format!("lnd-{}", instance.name),
                )])),
                ports: Some(vec![
                    k8s_openapi::api::core::v1::ServicePort {
                        name: Some("rest".to_string()),
                        port: rest_port,
                        target_port: Some(
                            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(8080),
                        ),
                        protocol: Some("TCP".to_string()),
                        ..Default::default()
                    },
                    k8s_openapi::api::core::v1::ServicePort {
                        name: Some("grpc".to_string()),
                        port: rest_port + 1,
                        target_port: Some(
                            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(10009),
                        ),
                        protocol: Some("TCP".to_string()),
                        ..Default::default()
                    },
                    k8s_openapi::api::core::v1::ServicePort {
                        name: Some("p2p".to_string()),
                        port: rest_port + 2,
                        target_port: Some(
                            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(9735),
                        ),
                        protocol: Some("TCP".to_string()),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }),
            status: None,
        };
        Api::<Service>::namespaced(self.client.clone(), ns)
            .create(&PostParams::default(), &svc)
            .await?;
        Ok(())
    }

    /// Public REST + gRPC addresses of one instance: the deployment's
    /// public host and the instance's LoadBalancer ports.
    pub fn lightning_rest_url(&self, rest_port: i32) -> String {
        format!("https://{}:{rest_port}", self.ln_host)
    }

    pub fn lightning_grpc_url(&self, rest_port: i32) -> String {
        format!("https://{}:{}", self.ln_host, rest_port + 1)
    }

    /// Peering URI of one instance: node id plus the host p2p port.
    pub fn lightning_node_uri(&self, rest_port: i32, node_id: &str) -> String {
        format!("{node_id}@{}:{}", self.ln_host, rest_port + 2)
    }

    /// Run one command in an instance's pod, returning stdout. A failing
    /// command must not return bytes: callers read the error as "not ready
    /// yet".
    async fn exec_in_pod(
        &self,
        env_id: &str,
        instance: &str,
        args: Vec<&str>,
    ) -> Result<Vec<u8>, OrchestrateError> {
        let ns = namespace_for(env_id);
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &ns);
        let command = args.join(" ");
        let mut attached = pods
            .exec(&lnd_pod_name(instance), args, &AttachParams::default())
            .await?;
        let mut stdout = attached
            .stdout()
            .ok_or_else(|| OrchestrateError::Exec("no stdout".to_string()))?;
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut buf)
            .await
            .map_err(|e| OrchestrateError::Exec(e.to_string()))?;
        // A failing command must not return bytes: callers read the
        // error as "not ready yet".
        let success = match attached.take_status() {
            Some(fut) => fut.await.and_then(|s| s.status).as_deref() == Some("Success"),
            None => false,
        };
        if !success {
            return Err(OrchestrateError::Exec(format!(
                "{command} in {} failed",
                lnd_pod_name(instance)
            )));
        }
        Ok(buf)
    }

    /// Read one file out of an instance's pod via exec.
    async fn read_pod_file(
        &self,
        env_id: &str,
        instance: &str,
        path: &str,
    ) -> Result<Vec<u8>, OrchestrateError> {
        self.exec_in_pod(env_id, instance, vec!["cat", path]).await
    }

    /// Read the admin macaroon out of the instance's pod (hex-encoded).
    /// Fails while the wallet is still being created — callers treat that
    /// as "not ready yet".
    pub async fn lightning_admin_macaroon(
        &self,
        env_id: &str,
        instance: &str,
    ) -> Result<String, OrchestrateError> {
        let buf = self
            .read_pod_file(env_id, instance, LND_MACAROON_PATH)
            .await?;
        Ok(encode_hex(&buf))
    }

    /// The instance's TLS CA (PEM). The orchestrator provisions it per
    /// instance; clients pin it as a root. Public material — only the
    /// macaroon is a secret.
    pub async fn lightning_tls_ca(
        &self,
        env_id: &str,
        instance: &str,
    ) -> Result<String, OrchestrateError> {
        let ns = namespace_for(env_id);
        let secrets: Api<Secret> = Api::namespaced(self.client.clone(), &ns);
        let secret = secrets.get(&format!("lnd-{instance}-tls")).await?;
        secret
            .data
            .as_ref()
            .and_then(|entries| entries.get("ca.crt"))
            .map(|bytes| String::from_utf8_lossy(&bytes.0).to_string())
            .ok_or_else(|| {
                OrchestrateError::Exec(format!("secret lnd-{instance}-tls has no ca.crt"))
            })
    }

    /// Identity public key (hex) of one instance's node, via
    /// `lncli getinfo` in the pod. Fails while the wallet is still being
    /// created — callers treat that as "not ready yet".
    pub async fn lightning_node_id(
        &self,
        env_id: &str,
        instance: &str,
    ) -> Result<String, OrchestrateError> {
        let buf = self
            .exec_in_pod(
                env_id,
                instance,
                vec!["lncli", "--network=signet", LND_TLS_CERT_FLAG, "getinfo"],
            )
            .await?;
        parse_node_id(&buf)
    }

    /// Initiate one p2p connection between two instances of the same
    /// environment. Envs are isolated chains (the signet magic derives
    /// from the per-env challenge), so public signet seeds can never
    /// peer — sibling instances are the only reachable peers.
    pub async fn lightning_connect_peer(
        &self,
        env_id: &str,
        instance: &str,
        peer_node_id: &str,
        peer_instance: &str,
    ) -> Result<(), OrchestrateError> {
        let target = format!("{peer_node_id}@lnd-{peer_instance}:9735");
        let mut args: Vec<&str> = lncli_base().to_vec();
        args.extend(["connect", &target]);
        self.exec_in_pod(env_id, instance, args).await?;
        Ok(())
    }

    /// Cross-connect every instance pair in one environment. Idempotent:
    /// pairs that are already peered are never re-connected (a repeated
    /// ConnectPeer makes lnd log an error). Returns the number of new
    /// connections initiated. Fails while any wallet is still being
    /// created — callers treat that as "not ready yet" and retry.
    pub async fn mesh_lightning_peers(
        &self,
        env_id: &str,
        instances: &[String],
    ) -> Result<usize, OrchestrateError> {
        if instances.len() < 2 {
            return Ok(0);
        }
        let mut node_ids = Vec::with_capacity(instances.len());
        for instance in instances {
            node_ids.push(self.lightning_node_id(env_id, instance).await?);
        }
        let mut initiated = 0;
        for (a_idx, a) in instances.iter().enumerate() {
            // Only connect pairs that are not peered yet: a repeated
            // ConnectPeer on a live connection makes lnd log an error
            // every reconcile pass (Polar checks first for the same
            // reason).
            let a_peers = self.lightning_peer_keys(env_id, a).await?;
            for (b, b_node_id) in instances.iter().zip(&node_ids).skip(a_idx + 1) {
                if a_peers.iter().any(|p| p == b_node_id) {
                    continue;
                }
                match self.lightning_connect_peer(env_id, a, b_node_id, b).await {
                    Ok(()) => initiated += 1,
                    // A concurrent connect wins the race: already-peered
                    // is success. Any other error propagates for retry.
                    Err(e) => {
                        let peers = self.lightning_peer_keys(env_id, a).await?;
                        if !peers.iter().any(|p| p == b_node_id) {
                            return Err(e);
                        }
                    }
                }
            }
        }
        Ok(initiated)
    }

    /// Pubkeys of one instance's current p2p peers.
    pub async fn lightning_peer_keys(
        &self,
        env_id: &str,
        instance: &str,
    ) -> Result<Vec<String>, OrchestrateError> {
        let mut args: Vec<&str> = lncli_base().to_vec();
        args.push("listpeers");
        let buf = self.exec_in_pod(env_id, instance, args).await?;
        parse_peer_keys(&buf)
    }

    /// Apply additional lightning instances into an existing env's
    /// namespace. `lnd_wallet_password` seeds the shared secret only when
    /// the key is absent — envs provisioned before the key existed must
    /// still accept new instances.
    pub async fn add_lightning_instances(
        &self,
        env_id: &str,
        instances: &[(LightningInstance, i32)],
        images: &BTreeMap<String, String>,
        lnd_wallet_password: &str,
    ) -> Result<(), OrchestrateError> {
        let ns = namespace_for(env_id);
        self.ensure_lnd_wallet_password(&ns, lnd_wallet_password)
            .await?;
        for (instance, rest_port) in instances {
            self.apply_lnd(&ns, instance, images, *rest_port).await?;
        }
        Ok(())
    }

    /// Patch the env secret with a wallet password if the key is missing.
    /// Existing instances must keep the password their wallet was created
    /// with, so a present key is never touched.
    async fn ensure_lnd_wallet_password(
        &self,
        ns: &str,
        password: &str,
    ) -> Result<(), OrchestrateError> {
        let secrets: Api<Secret> = Api::namespaced(self.client.clone(), ns);
        let secret = secrets.get(SECRET_NAME).await?;
        if secret
            .data
            .as_ref()
            .is_some_and(|data| data.contains_key(LND_WALLET_PASSWORD_KEY))
        {
            return Ok(());
        }
        let patch = Patch::Merge(json!({
            "stringData": { LND_WALLET_PASSWORD_KEY: password },
        }));
        secrets
            .patch(SECRET_NAME, &PatchParams::default(), &patch)
            .await?;
        Ok(())
    }

    pub async fn destroy_environment(&self, env_id: &str) -> Result<(), OrchestrateError> {
        self.delete_env_namespace(&namespace_for(env_id)).await
    }

    /// Whether the environment's namespace still exists. Handlers use this
    /// to reconcile rows whose cluster state a reap removed behind their back.
    pub async fn environment_exists(&self, env_id: &str) -> Result<bool, OrchestrateError> {
        let ns = Api::<Namespace>::all(self.client.clone())
            .get_opt(&namespace_for(env_id))
            .await?;
        Ok(ns.is_some())
    }

    /// Scale every workload in the environment namespace. `start` assumes
    /// one replica per workload, which is all the stack ever runs.
    pub async fn set_workload_replicas(
        &self,
        env_id: &str,
        replicas: i32,
    ) -> Result<(), OrchestrateError> {
        let ns = namespace_for(env_id);
        self.scale_all::<StatefulSet>(&ns, replicas).await?;
        self.scale_all::<Deployment>(&ns, replicas).await?;
        Ok(())
    }

    async fn scale_all<K>(&self, ns: &str, replicas: i32) -> Result<(), OrchestrateError>
    where
        K: kube::Resource<Scope = kube::core::NamespaceResourceScope>,
        K::DynamicType: Default,
        K: Clone + serde::de::DeserializeOwned + std::fmt::Debug,
    {
        let api = Api::<K>::namespaced(self.client.clone(), ns);
        for item in api.list(&ListParams::default()).await? {
            let Some(name) = item.meta().name.clone() else {
                continue;
            };
            api.patch(
                &name,
                &PatchParams::default(),
                &Patch::Strategic(&json!({ "spec": { "replicas": replicas } })),
            )
            .await?;
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
        let patch = container_images(images);
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
                        patch_images(&mut pod.containers, &patch);
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
                "PersistentVolumeClaim" => {
                    let mut pvc: k8s_openapi::api::core::v1::PersistentVolumeClaim =
                        serde_yaml::from_value(value)?;
                    pvc.metadata.namespace = Some(ns.to_string());
                    Api::namespaced(self.client.clone(), ns)
                        .create(&PostParams::default(), &pvc)
                        .await?;
                }
                "Deployment" => {
                    let mut dep: Deployment = serde_yaml::from_value(value)?;
                    dep.metadata.namespace = Some(ns.to_string());
                    if let Some(pod) = dep.spec.as_mut().and_then(|s| s.template.spec.as_mut()) {
                        patch_images(&mut pod.containers, &patch);
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
    use signet_core::env::LightningImplementation;

    #[test]
    fn lnd_manifest_mounts_the_instance_tls_secret() {
        let manifest = lnd_manifest(&LightningInstance {
            name: "ln-smoke".to_string(),
            implementation: LightningImplementation::Lnd,
        });
        assert!(manifest.contains("secretName: lnd-ln-smoke-tls"));
        assert!(manifest.contains("--tlscertpath=/root/.lnd/tls/tls.crt"));
        assert!(manifest.contains("--tlskeypath=/root/.lnd/tls/tls.key"));
        assert!(manifest.contains("--nobootstrap"));
        assert!(!manifest.contains("EXTRA_HOST"));
        assert!(!manifest.contains("tlsextra"));
    }

    #[test]
    fn lnd_exec_targets_the_statefulset_pod() {
        assert_eq!(lnd_pod_name("ln-alice"), "lnd-ln-alice-0");
    }

    #[test]
    fn getinfo_yields_the_identity_key() {
        let out = br#"{
            "version": "0.21.4-beta",
            "identity_pubkey": "02aa550000000000000000000000000000000000000000000000000000000000"
        }"#;
        assert_eq!(
            parse_node_id(out).unwrap(),
            "02aa550000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn malformed_getinfo_output_is_an_error() {
        assert!(parse_node_id(b"{}").is_err());
        assert!(parse_node_id(b"not json").is_err());
    }

    #[test]
    fn listpeers_yields_peer_pubkeys() {
        let out = br#"{"peers": [{"pub_key": "02aa", "address": "10.42.0.21:9735"},
                             {"pub_key": "03bb", "address": "10.42.0.22:9735"}]}"#;
        assert_eq!(parse_peer_keys(out).unwrap(), vec!["02aa", "03bb"]);
    }

    #[test]
    fn listpeers_without_peers_is_empty() {
        assert_eq!(
            parse_peer_keys(br#"{"peers": []}"#).unwrap(),
            Vec::<String>::new()
        );
        assert!(parse_peer_keys(b"not json").is_err());
    }

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
    fn ttl_stamps_expires_at_annotation_on_namespace() {
        let expires = Utc::now() + chrono::Duration::seconds(1200);
        let ns = namespace_object("9f2a1b", Some(expires));
        let annotations = ns.metadata.annotations.unwrap();
        let raw = annotations
            .get(EXPIRES_AT_ANNOTATION)
            .expect("expires-at annotation");
        let parsed: DateTime<Utc> = DateTime::parse_from_rfc3339(raw).unwrap().into();
        assert_eq!(parsed, expires);
        assert_eq!(ns.metadata.name.as_deref(), Some("env-9f2a1b"));
    }

    #[test]
    fn no_ttl_leaves_namespace_annotations_off() {
        let ns = namespace_object("9f2a1b", None);
        assert!(ns.metadata.annotations.is_none());
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

    fn catalog() -> VersionCatalog {
        VersionCatalog::embedded()
    }

    #[test]
    fn resolve_images_defaults_without_request() {
        let images = resolve_images(&catalog(), &None).unwrap();
        for (key, image) in [
            ("bitcoind", "docker.io/bitcoin/bitcoin:29.4"),
            ("signer", "signet-signer:dev"),
            ("faucet", "signet-faucet:dev"),
            ("electrs", "ghcr.io/nully0x/electrs:0.11.1"),
            ("explorer", "docker.io/mempool/backend:v3.3.1"),
            ("lnd", "docker.io/lightninglabs/lnd:v0.21.4-beta"),
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
    fn explorer_version_pins_backend_and_frontend_together() {
        let images = resolve_images(
            &catalog(),
            &Some(BTreeMap::from([(
                "explorer".to_string(),
                "v3.3.1".to_string(),
            )])),
        )
        .unwrap();
        let patch = container_images(&images);
        assert_eq!(
            patch.get("explorer").unwrap(),
            "docker.io/mempool/backend:v3.3.1"
        );
        assert_eq!(
            patch.get("explorer-web").unwrap(),
            "docker.io/mempool/frontend:v3.3.1"
        );

        let docs = Orchestrator::parse_docs(EXPLORER_MANIFEST).unwrap();
        let mut deployment: Deployment = serde_yaml::from_value(docs[0].clone()).unwrap();
        let pod = deployment
            .spec
            .as_mut()
            .unwrap()
            .template
            .spec
            .as_mut()
            .unwrap();
        patch_images(&mut pod.containers, &patch);
        for (name, image) in [
            ("explorer", "docker.io/mempool/backend:v3.3.1"),
            ("explorer-web", "docker.io/mempool/frontend:v3.3.1"),
        ] {
            let container = pod.containers.iter().find(|c| c.name == name).unwrap();
            assert_eq!(container.image.as_deref(), Some(image));
        }
    }

    #[test]
    fn patch_images_renames_electrs_container_image() {
        let mut containers = vec![k8s_openapi::api::core::v1::Container {
            name: "electrs".into(),
            ..Default::default()
        }];
        let images = resolve_images(
            &catalog(),
            &Some(BTreeMap::from([(
                "electrs".to_string(),
                "0.10.9".to_string(),
            )])),
        )
        .unwrap();
        patch_images(&mut containers, &images);
        assert_eq!(
            containers[0].image.as_deref(),
            Some("ghcr.io/nully0x/electrs:0.10.9")
        );
    }

    #[test]
    fn resolve_images_versions_lnd_and_explorer_tags() {
        let tags = Some(BTreeMap::from([
            ("explorer".to_string(), "v3.3.1".to_string()),
            ("lnd".to_string(), "v0.20.5-beta".to_string()),
        ]));
        let images = resolve_images(&catalog(), &tags).unwrap();
        assert_eq!(
            images.get("explorer").unwrap(),
            "docker.io/mempool/backend:v3.3.1"
        );
        assert_eq!(
            images.get("lnd").unwrap(),
            "docker.io/lightninglabs/lnd:v0.20.5-beta"
        );
        assert_eq!(images.get("signer").unwrap(), "signet-signer:dev");
    }

    #[test]
    fn resolve_images_rejects_unknown_component() {
        for component in ["nginx", "faucet", "signer"] {
            let tags = Some(BTreeMap::from([(component.to_string(), "1.0".to_string())]));
            assert!(
                matches!(
                    resolve_images(&catalog(), &tags),
                    Err(VersionError::Incompatible(_))
                ),
                "{component}"
            );
        }
    }

    #[test]
    fn resolve_images_rejects_versions_outside_the_catalog() {
        let tags = Some(BTreeMap::from([(
            "bitcoind".to_string(),
            "28.1".to_string(),
        )]));
        let err = resolve_images(&catalog(), &tags).unwrap_err();
        assert!(err.to_string().contains("unknown version"), "{err}");
    }

    #[test]
    fn resolve_images_rejects_bad_tags() {
        for tag in ["", "a b", "$(id)", &"x".repeat(65)] {
            let tags = Some(BTreeMap::from([("bitcoind".to_string(), tag.to_string())]));
            assert!(
                matches!(
                    resolve_images(&catalog(), &tags),
                    Err(VersionError::BadTag(_))
                ),
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
    fn ln_host_override_wins_and_defaults_to_the_env_host() {
        assert_eq!(resolve_ln_host("http://localhost", None), "localhost");
        assert_eq!(resolve_ln_host("http://localhost", Some("")), "localhost");
        assert_eq!(
            resolve_ln_host("http://localhost", Some("192.168.100.2")),
            "192.168.100.2"
        );
        // Scheme and port are stripped: bundle URLs append their own port.
        assert_eq!(
            resolve_ln_host(
                "https://sandbox.signet.dev",
                Some("http://ln.signet.dev:9999")
            ),
            "ln.signet.dev"
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
