// Local mode bootstrap: one long-lived k3d cluster hosting the API and the
// environment namespaces. Mirrors `just cluster-up`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Context as _;
use nostr::nips::nip19::ToBech32 as _;

pub const CLUSTER: &str = "signet";
pub const DEFAULT_LOCAL_API: &str = "http://localhost:8081";
pub const EG_VERSION: &str = "v1.2.4";
pub const GATEWAY_MANIFEST: &str = include_str!("../templates/gateway-eg.yaml");
pub const LOCAL_STACK_MANIFEST: &str = include_str!("../templates/local-stack.yaml");
pub const PLATFORM_NAMESPACE: &str = "signet-platform";

/// Default publish target of the `publish-images` workflow. Override for
/// forks with SIGNET_IMAGE_REGISTRY.
fn registry() -> String {
    std::env::var("SIGNET_IMAGE_REGISTRY").unwrap_or_else(|_| "ghcr.io/nully0x".to_string())
}

fn image(name: &str) -> String {
    full_image(&registry(), name)
}

/// Auto-bootstrap applies only when the caller targets the default local
/// api and it is not reachable; a custom `--api` that is down is an error.
pub fn should_bootstrap(api: &str, reachable: bool) -> bool {
    api == DEFAULT_LOCAL_API && !reachable
}

/// The profile token applies only when no explicit credential was given
/// (flag or env); an empty value counts as unset.
pub fn profile_credentials_apply(common: &crate::Common) -> bool {
    common.token.as_deref().is_none_or(str::is_empty)
        && common.nsec.as_deref().is_none_or(str::is_empty)
}

/// Every image an environment manifest can reference: (name to pull, local
/// retags). Names without a `/` are ours and live under the registry; names
/// with a `/` are upstream (docker.io) and are pulled as-is. The retags map
/// registry pulls onto the `:dev` tags the env templates reference.
pub const IMAGES: &[(&str, &[&str])] = &[
    ("signet-api:latest", &[]),
    ("signet-signer:latest", &["signet-signer:dev"]),
    ("signet-faucet:latest", &["signet-faucet:dev"]),
    ("electrs:latest", &["electrs:dev", "electrs:0.11.1"]),
    ("bitcoin/bitcoin:29.4", &[]),
    ("mempool/backend:v3.3.1", &[]),
    ("mempool/frontend:v3.3.1", &[]),
];

fn full_image(registry: &str, name: &str) -> String {
    if name.contains('/') {
        name.to_string()
    } else {
        format!("{registry}/{name}")
    }
}

const NODE_READY_TIMEOUT_SECS: u64 = 180;

pub fn bootstrap() -> anyhow::Result<()> {
    let bin = ensure_prerequisites()?;
    ensure_cluster(&bin)?;
    ensure_images()?;
    install_gateway(bin.as_path())?;
    deploy_local_stack(bin.as_path())?;
    wait_api()?;
    let profile = crate::profile::ensure_with(
        &crate::profile::path()?,
        "http://localhost:8081",
        crate::mint_token,
    )?;
    let npub = nostr::key::Keys::parse(&profile.nsec)
        .map(|keys| keys.public_key().to_bech32().unwrap_or_default())
        .unwrap_or_default();
    println!(
        "profile written: {} (identity {npub})",
        crate::profile::path()?.display()
    );
    println!("local stack ready: provisioning api on http://localhost:8081");
    Ok(())
}

/// Report cluster, api, and profile state without changing anything.
pub fn status() -> anyhow::Result<()> {
    let bin = bin_dir()?;
    let cluster = k3d(&bin)?
        .args(["cluster", "get", CLUSTER])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    println!(
        "cluster: {}",
        if cluster {
            "running"
        } else {
            "not bootstrapped"
        }
    );
    let api = api_healthy(DEFAULT_LOCAL_API);
    println!(
        "api: {}",
        if api {
            "healthy on http://localhost:8081"
        } else {
            "unreachable"
        }
    );
    match crate::profile::load() {
        Ok(profile) => {
            let npub = nostr::key::Keys::parse(&profile.nsec)
                .ok()
                .and_then(|keys| keys.public_key().to_bech32().ok())
                .unwrap_or_else(|| "<unparsable>".to_string());
            println!("profile: identity {npub}");
        }
        Err(_) => println!("profile: none — run signet local init"),
    }
    Ok(())
}

/// Tear the local cluster down. The profile survives: the identity is
/// reused and the token re-minted by the next `signet local init`.
pub fn teardown() -> anyhow::Result<()> {
    let bin = bin_dir()?;
    run(k3d(&bin)?.args(["cluster", "delete", CLUSTER]))?;
    println!("cluster `{CLUSTER}` deleted (profile kept at ~/.signet/config.toml)");
    Ok(())
}

fn ensure_prerequisites() -> anyhow::Result<PathBuf> {
    if !Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        anyhow::bail!(
            "docker daemon is not reachable — install docker for your platform and start it"
        );
    }
    let bin = bin_dir()?;
    std::fs::create_dir_all(&bin)?;
    Ok(bin)
}

fn bin_dir() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(Path::new(&home).join(".signet").join("bin"))
}

fn have_on_path(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fetch_text(url: &str) -> anyhow::Result<String> {
    Ok(reqwest::blocking::get(url)?.error_for_status()?.text()?)
}

fn fetch_bytes(url: &str) -> anyhow::Result<Vec<u8>> {
    Ok(reqwest::blocking::get(url)?
        .error_for_status()?
        .bytes()?
        .to_vec())
}

fn k3d_download_url() -> String {
    format!(
        "https://github.com/k3d-io/k3d/releases/latest/download/k3d-{}-{}",
        os_name(),
        arch_name()
    )
}

fn kubectl_version_url() -> String {
    "https://dl.k8s.io/release/stable.txt".to_string()
}

fn kubectl_download_url() -> String {
    format!(
        "https://dl.k8s.io/release/{{version}}/bin/{}/{}/kubectl",
        os_name(),
        arch_name()
    )
}

fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn arch_name() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// Resolve a bootstrap tool into a ready Command. The vendored copy in the
/// bin dir wins, then PATH; neither present, the binary is downloaded into
/// the bin dir. The resolution must match how the command is built — a
/// download-skip must never leave the command pointing at a missing file.
fn tool(
    bin: &Path,
    name: &str,
    download_url: &str,
    version_url: Option<&str>,
) -> anyhow::Result<Command> {
    let vendored = bin.join(name);
    if !vendored.exists() && !have_on_path(name) {
        println!("downloading {name} into {}", bin.display());
        let url = match version_url {
            Some(pattern) => {
                let version = fetch_text(pattern)?;
                download_url.replace("{version}", version.trim())
            }
            None => download_url.to_string(),
        };
        let body = fetch_bytes(&url)?;
        std::fs::write(&vendored, &body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&vendored, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    let program = if vendored.exists() {
        vendored
    } else {
        PathBuf::from(name)
    };
    let mut cmd = Command::new(program);
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    Ok(cmd)
}

fn k3d(bin: &Path) -> anyhow::Result<Command> {
    tool(bin, "k3d", &k3d_download_url(), None)
}

fn kubectl(bin: &Path) -> anyhow::Result<Command> {
    tool(
        bin,
        "kubectl",
        &kubectl_download_url(),
        Some(&kubectl_version_url()),
    )
}

fn ensure_cluster(bin: &Path) -> anyhow::Result<()> {
    let exists = k3d(bin)?
        .args(["cluster", "get", CLUSTER])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if exists {
        println!("cluster exists; starting");
        run(k3d(bin)?.args(["cluster", "start", CLUSTER]))?;
    } else {
        println!("creating k3d cluster `{CLUSTER}`");
        run(k3d(bin)?
            .args(["cluster", "create", CLUSTER])
            .arg("--api-port")
            .arg("34611")
            .arg("--port")
            .arg("80:80@loadbalancer")
            .arg("--port")
            .arg("443:443@loadbalancer")
            .arg("--port")
            .arg("8081:8081@loadbalancer")
            .arg("--port")
            .arg("20000-20599:20000-20599@loadbalancer")
            .arg("--port")
            .arg("21000-21199:21000-21199@loadbalancer")
            .arg("--volume")
            .arg("signet-data:/var/lib/rancher/k3s/storage@server:0"))?;
        run(k3d(bin)?.args(["kubeconfig", "write", CLUSTER]))?;
    }

    wait_for_node(bin)?;
    Ok(())
}

fn wait_for_node(bin: &Path) -> anyhow::Result<()> {
    println!("waiting for node ready…");
    for _ in 0..(NODE_READY_TIMEOUT_SECS / 2) {
        let ready = kubectl(bin)?
            .args([
                "get",
                "nodes",
                "-o",
                "jsonpath={range .items[*]}{.status.conditions[?(@.type==\"Ready\")].status}{end}",
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("True"))
            .unwrap_or(false);
        if ready {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    anyhow::bail!("node never became ready after {NODE_READY_TIMEOUT_SECS}s")
}

/// The refs that must exist in the node for one entry: the retag names
/// when present, otherwise the pulled reference itself. Retags are
/// normalized the way kubelet does it — a repo without a registry lives on
/// docker.io under `library` — or the exact-name lookup never matches.
fn required_refs(name: &str, retags: &[&str]) -> Vec<String> {
    if retags.is_empty() {
        if name.contains('/') {
            vec![format!("docker.io/{name}")]
        } else {
            vec![image(name)]
        }
    } else {
        retags.iter().map(|t| fully_qualified(t)).collect()
    }
}

/// Docker's name normalization: a repo with a path component lives under
/// its registry as-is; a bare repo lands in docker.io/library.
fn fully_qualified(tag: &str) -> String {
    if tag.contains('/') {
        tag.to_string()
    } else {
        format!("docker.io/library/{tag}")
    }
}

fn node_image_list(node: &str, socket: &str) -> String {
    let mut out = Command::new("docker")
        .args([
            "exec", node, "ctr", "-n", "k8s.io", "-a", socket, "images", "ls", "-q",
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    out.push('\n');
    out
}

/// Run a command capturing output; stdout stays quiet, stderr is reported
/// on failure. Returns the captured stdout.
fn run_capture(cmd: &mut Command) -> anyhow::Result<String> {
    let out = cmd.output()?;
    anyhow::ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Ensure every image the env manifests reference exists in the node's
/// containerd. Resolution order per image: already in the node, available
/// on the host docker (transferred without touching the registry), then a
/// quiet remote pull. Per-image errors are reported as they happen; the
/// command fails at the end if anything is unresolved.
fn ensure_images() -> anyhow::Result<()> {
    let node = format!("k3d-{CLUSTER}-server-0");
    let socket = "/run/k3s/containerd/containerd.sock";
    let mut listed = node_image_list(&node, socket);

    let total = IMAGES.len();
    let mut failed: Vec<String> = Vec::new();

    for (index, (name, retags)) in IMAGES.iter().enumerate() {
        let required = required_refs(name, retags);
        let satisfied = required.iter().all(|r| listed.contains(&format!("{r}\n")));
        let label = required.join(", ");
        if satisfied {
            println!("[{}/{total}] {label} — already in the node", index + 1);
            continue;
        }

        // Host docker first: transfer without touching the registry.
        let host_candidates: Vec<String> = if name.contains('/') {
            vec![name.to_string()]
        } else {
            vec![name.to_string(), image(name)]
        };
        let host_ref = host_candidates
            .iter()
            .find(|r| {
                Command::new("docker")
                    .args(["image", "inspect", r])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            })
            .cloned();

        let mut done = false;
        if let Some(host_ref) = &host_ref {
            println!(
                "[{}/{total}] {label} — from local docker ({host_ref})",
                index + 1
            );
            let transfer = || -> anyhow::Result<()> {
                let tar = std::env::temp_dir().join("signet-image-transfer.tar");
                run_capture(
                    Command::new("docker")
                        .args(["save", "-o"])
                        .arg(&tar)
                        .arg(host_ref),
                )?;
                run_capture(Command::new("docker").args([
                    "cp",
                    &tar.to_string_lossy(),
                    &format!("{node}:/tmp/signet-transfer.tar"),
                ]))?;
                run_capture(Command::new("docker").args(["exec", &node, "sh", "-c",
                    "ctr -n k8s.io -a /run/k3s/containerd/containerd.sock images import /tmp/signet-transfer.tar"]))?;
                // containerd normalizes the imported name; tag from that.
                let imported_ref = fully_qualified(host_ref);
                for r in &required {
                    if r != &imported_ref {
                        run_capture(Command::new("docker").args([
                            "exec",
                            &node,
                            "ctr",
                            "-n",
                            "k8s.io",
                            "-a",
                            "/run/k3s/containerd/containerd.sock",
                            "images",
                            "tag",
                            &imported_ref,
                            r,
                        ]))?;
                    }
                }
                let _ = std::fs::remove_file(&tar);
                Ok(())
            };
            match transfer() {
                Ok(()) => {
                    for r in &required {
                        listed.push_str(&format!("{r}\n"));
                    }
                    done = true;
                }
                Err(e) => println!("  host transfer failed ({e}); falling back to the registry"),
            }
        }

        // Remote pull into the node's containerd.
        if !done {
            let source = if name.contains('/') {
                format!("docker.io/{name}")
            } else {
                image(name)
            };
            println!("[{}/{total}] {label} — pulling {source}", index + 1);
            match ctr_quiet(&node, &["pull", &source]) {
                Ok(_) => {
                    for r in &required {
                        if !listed.contains(&format!("{r}\n")) {
                            ctr_quiet(&node, &["tag", &source, r])?;
                            listed.push_str(&format!("{r}\n"));
                        }
                    }
                }
                Err(e) => {
                    println!("  pull failed: {e}");
                    failed.push(label.clone());
                    continue;
                }
            }
        }
    }

    if !failed.is_empty() {
        anyhow::bail!("images failed to resolve: {failed:?}");
    }
    Ok(())
}

fn ctr_quiet(node: &str, args: &[&str]) -> anyhow::Result<()> {
    let mut cmd = Command::new("docker");
    cmd.args(["exec", node, "ctr", "-n", "k8s.io", "-a"])
        .arg("/run/k3s/containerd/containerd.sock")
        .args(["images"])
        .args(args);
    run_capture(&mut cmd)?;
    Ok(())
}

/// Apply the postgres + api stack and wait for both rollouts. The registry
/// placeholder is substituted so forks can point SIGNET_IMAGE_REGISTRY at
/// their own ghcr namespace.
fn deploy_local_stack(bin: &Path) -> anyhow::Result<()> {
    println!("deploying postgres + provisioning api");
    let manifest = LOCAL_STACK_MANIFEST.replace("REGISTRY_PLACEHOLDER", &registry());
    write_stdin_manifest(bin, &manifest)?;
    run(kubectl(bin)?.args([
        "-n",
        PLATFORM_NAMESPACE,
        "rollout",
        "status",
        "deploy/postgres",
        "--timeout=180s",
    ]))?;
    run(kubectl(bin)?.args([
        "-n",
        PLATFORM_NAMESPACE,
        "rollout",
        "status",
        "deploy/signet-api",
        "--timeout=180s",
    ]))?;
    Ok(())
}

/// Probe the api health endpoint with a short timeout; used by `local
/// status`, `wait_api`, and the auto-bootstrap decision.
pub(crate) fn api_healthy(url: &str) -> bool {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()
        .and_then(|client| {
            client
                .get(format!("{url}/healthz"))
                .send()
                .ok()
                .map(|r| r.status().is_success())
        })
        .unwrap_or(false)
}

/// The k3d serverlb maps host 8081 to the api's LoadBalancer service; poll
/// until the routing chain answers.
fn wait_api() -> anyhow::Result<()> {
    println!("waiting for the api on {DEFAULT_LOCAL_API}…");
    for _ in 0..30 {
        if api_healthy(DEFAULT_LOCAL_API) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    anyhow::bail!("api did not become healthy on {DEFAULT_LOCAL_API} within 60s")
}

/// Drop CustomResourceDefinition documents from a multi-doc manifest.
/// k3s manages the gateway-api CRDs itself; EG's bundled copies must not
/// overwrite them.
fn strip_crd_docs(manifest: &str) -> String {
    use serde::Deserialize as _;
    let docs: Vec<serde_yaml::Value> = serde_yaml::Deserializer::from_str(manifest)
        .map(|doc| serde_yaml::Value::deserialize(doc).unwrap())
        .filter(|doc| doc["kind"].as_str() != Some("CustomResourceDefinition"))
        .collect();
    docs.iter()
        .map(|doc| serde_yaml::to_string(doc).unwrap())
        .collect::<Vec<_>>()
        .join("---\n")
}

fn install_gateway(bin: &Path) -> anyhow::Result<()> {
    println!("installing the envoy gateway stack");
    // k3s installs its own gateway-api CRDs at server boot; EG's bundled
    // CRD copies conflict with them (different version sets). Apply the
    // installer minus CRD documents — the controller runs on the CRDs k3s
    // already provides.
    let eg_install = fetch_text(&format!(
        "https://github.com/envoyproxy/gateway/releases/download/{EG_VERSION}/install.yaml"
    ))?;
    write_stdin_manifest(bin, &strip_crd_docs(&eg_install))?;
    run(kubectl(bin)?.args([
        "-n",
        "envoy-gateway-system",
        "rollout",
        "status",
        "deploy/envoy-gateway",
        "--timeout=240s",
    ]))?;
    write_stdin_manifest(bin, GATEWAY_MANIFEST)?;
    run(kubectl(bin)?.args([
        "wait",
        "--for=condition=accepted",
        "gatewayclass/signet-eg",
        "--timeout=120s",
    ]))?;
    Ok(())
}

/// `kubectl apply -f -` from an embedded manifest; local init cannot assume
/// the repo checkout exists on the machine.
fn write_stdin_manifest(bin: &Path, manifest: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    let mut child = kubectl(bin)?
        .args(["apply", "--server-side", "--force-conflicts", "-f", "-"])
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .context("kubectl stdin")?
        .write_all(manifest.as_bytes())?;
    let status = child.wait()?;
    anyhow::ensure!(status.success(), "kubectl apply failed");
    Ok(())
}

fn run(cmd: &mut Command) -> anyhow::Result<()> {
    let status = cmd.status()?;
    anyhow::ensure!(status.success(), "command failed: {:?}", cmd);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_urls_match_platform() {
        let url = k3d_download_url();
        assert!(url.starts_with("https://github.com/k3d-io/k3d/releases/latest/download/k3d-"));
        assert!(
            url.ends_with("-linux-amd64") || url.ends_with("-linux-arm64") || url.contains("macos")
        );
        let kubectl = kubectl_download_url();
        assert!(kubectl.contains("{version}"));
        assert!(
            kubectl.contains("/bin/linux/amd64/")
                || kubectl.contains("/bin/linux/arm64/")
                || kubectl.contains("darwin")
        );
    }

    #[test]
    fn gateway_manifest_embeds_the_platform_gateway() {
        assert!(GATEWAY_MANIFEST.contains("name: signet-eg"));
        assert!(GATEWAY_MANIFEST.contains("kind: Gateway"));
    }

    #[test]
    fn image_list_covers_every_manifest_reference() {
        let mut names: Vec<&str> = Vec::new();
        for (name, retags) in IMAGES {
            names.push(name);
            names.extend(retags.iter().copied());
        }
        names.sort_unstable();
        names.dedup();
        for name in names {
            assert!(name.contains(':'), "{name} must pin a tag");
        }
        // The env templates reference these exact tags.
        for required in ["signet-signer:dev", "electrs:dev", "electrs:0.11.1"] {
            assert!(
                IMAGES.iter().any(|(_, retags)| retags.contains(&required)),
                "{required} missing"
            );
        }
    }

    #[test]
    fn full_image_prefixes_only_registry_images() {
        assert_eq!(
            full_image("ghcr.io/nully0x", "signet-api:latest"),
            "ghcr.io/nully0x/signet-api:latest"
        );
        assert_eq!(
            full_image("ghcr.io/nully0x", "electrs:latest"),
            "ghcr.io/nully0x/electrs:latest"
        );
        assert_eq!(
            full_image("ghcr.io/nully0x", "bitcoin/bitcoin:29.4"),
            "bitcoin/bitcoin:29.4"
        );
        assert_eq!(
            full_image("example.com/mine", "mempool/backend:v3.3.1"),
            "mempool/backend:v3.3.1"
        );
    }

    #[test]
    fn local_stack_manifest_is_complete_multi_doc_yaml() {
        // Never split or hand-consume multi-doc yaml (see repo gotcha 13);
        // kubectl applies the file as one stream, so parse it the same way.
        use serde::Deserialize as _;
        let docs: Vec<serde_yaml::Value> = serde_yaml::Deserializer::from_str(LOCAL_STACK_MANIFEST)
            .map(|doc| serde_yaml::Value::deserialize(doc).unwrap())
            .collect();
        let kinds: Vec<String> = docs
            .iter()
            .map(|d| d["kind"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            kinds,
            [
                "Service",
                "PersistentVolumeClaim",
                "Deployment",
                "ServiceAccount",
                "ClusterRole",
                "ClusterRoleBinding",
                "Service",
                "Deployment"
            ]
        );
        assert!(LOCAL_STACK_MANIFEST.contains("image: signet-api:latest"));
        assert!(LOCAL_STACK_MANIFEST.contains("localhost:8081"));
    }
}
