// Local mode bootstrap: one long-lived k3d cluster hosting the API and the
// environment namespaces (spec §10.2). Mirrors `just cluster-up`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Context as _;

pub const CLUSTER: &str = "signet";
pub const EG_VERSION: &str = "v1.2.4";
pub const GATEWAY_API_VERSION: &str = "v1.2.1";
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

pub fn init() -> anyhow::Result<()> {
    let bin = ensure_prerequisites()?;
    ensure_binary(&bin, "k3d", &k3d_download_url(), None)?;
    ensure_binary(
        &bin,
        "kubectl",
        &kubectl_download_url(),
        Some(&kubectl_version_url()),
    )?;
    ensure_cluster(&bin)?;
    pull_images(bin.as_path())?;
    install_gateway(bin.as_path())?;
    deploy_local_stack(bin.as_path())?;
    wait_api()?;
    println!("local stack ready: provisioning api on http://localhost:8081");
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

fn ensure_binary(
    bin: &Path,
    name: &str,
    download_url: &str,
    version_url: Option<&str>,
) -> anyhow::Result<()> {
    if have_on_path(name) || bin.join(name).exists() {
        return Ok(());
    }
    let url = match version_url {
        Some(pattern) => {
            let version = fetch_text(pattern)?;
            download_url.replace("{version}", version.trim())
        }
        None => download_url.to_string(),
    };
    println!("downloading {name} into {}", bin.display());
    let body = fetch_bytes(&url)?;
    let target = bin.join(name);
    std::fs::write(&target, &body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
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

fn k3d(bin: &Path) -> Command {
    let mut cmd = Command::new(bin.join("k3d"));
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    cmd
}

fn kubectl(bin: &Path) -> Command {
    let mut cmd = Command::new(bin.join("kubectl"));
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    cmd
}

fn ensure_cluster(bin: &Path) -> anyhow::Result<()> {
    let exists = k3d(bin)
        .args(["cluster", "get", CLUSTER])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if exists {
        println!("cluster exists; starting");
        run(k3d(bin).args(["cluster", "start", CLUSTER]))?;
    } else {
        println!("creating k3d cluster `{CLUSTER}`");
        run(k3d(bin)
            .args(["cluster", "create", CLUSTER])
            .arg("--port")
            .arg("80:80@loadbalancer")
            .arg("--port")
            .arg("443:443@loadbalancer")
            .arg("--port")
            .arg("8081:8081@loadbalancer")
            .arg("--port")
            .arg("50002-50011:50002-50011@loadbalancer")
            .arg("--volume")
            .arg("signet-data:/var/lib/rancher/k3s/storage@server:0"))?;
        run(k3d(bin).args(["kubeconfig", "write", CLUSTER]))?;
    }

    wait_for_node(bin)?;
    Ok(())
}

fn wait_for_node(bin: &Path) -> anyhow::Result<()> {
    println!("waiting for node ready…");
    for _ in 0..(NODE_READY_TIMEOUT_SECS / 2) {
        let ready = kubectl(bin)
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

/// Pull every image the env manifests reference and import them into the
/// k3d node. The import is verified inside the node: `k3d image import`
/// can succeed without importing anything.
fn pull_images(bin: &Path) -> anyhow::Result<()> {
    let mut local_names: Vec<String> = Vec::new();
    for (name, retags) in IMAGES {
        let full = image(name);
        println!("pulling {full}");
        run(Command::new("docker").args(["pull", &full])).map_err(|e| {
            anyhow::anyhow!("pulling {full} failed: {e} — has the publish-images workflow run?")
        })?;
        for tag in *retags {
            run(Command::new("docker").args(["tag", &full, tag]))?;
            local_names.push(tag.to_string());
        }
        local_names.push(full);
    }
    import_images(bin, &local_names)
}

fn import_images(bin: &Path, names: &[String]) -> anyhow::Result<()> {
    println!("importing {} images into the {CLUSTER} node", names.len());
    let mut cmd = k3d(bin);
    cmd.args(["image", "import"]);
    for name in names {
        cmd.arg(name);
    }
    cmd.args(["-c", CLUSTER]);
    run(&mut cmd)?;

    // Gotcha: k3d image import can silently no-op. Verify inside the node.
    let node = format!("k3d-{CLUSTER}-server-0");
    let listed = Command::new("docker")
        .args(["exec", &node, "crictl", "img"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let missing: Vec<&String> = names
        .iter()
        .filter(|n| !listed.contains(n.as_str()))
        .collect();
    anyhow::ensure!(
        missing.is_empty(),
        "images did not land in the node: {missing:?}"
    );
    Ok(())
}

/// Apply the postgres + api stack and wait for both rollouts. The registry
/// placeholder is substituted so forks can point SIGNET_IMAGE_REGISTRY at
/// their own ghcr namespace.
fn deploy_local_stack(bin: &Path) -> anyhow::Result<()> {
    println!("deploying postgres + provisioning api");
    let manifest = LOCAL_STACK_MANIFEST.replace("REGISTRY_PLACEHOLDER", &registry());
    write_stdin_manifest(bin, &manifest)?;
    run(kubectl(bin).args([
        "-n",
        PLATFORM_NAMESPACE,
        "rollout",
        "status",
        "deploy/postgres",
        "--timeout=180s",
    ]))?;
    run(kubectl(bin).args([
        "-n",
        PLATFORM_NAMESPACE,
        "rollout",
        "status",
        "deploy/signet-api",
        "--timeout=180s",
    ]))?;
    Ok(())
}

/// The k3d serverlb maps host 8081 to the api's LoadBalancer service; poll
/// until the routing chain answers.
fn wait_api() -> anyhow::Result<()> {
    println!("waiting for the api on http://localhost:8081…");
    for _ in 0..30 {
        if reqwest::blocking::get("http://localhost:8081/healthz")
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    anyhow::bail!("api did not become healthy on localhost:8081 within 60s")
}

fn install_gateway(bin: &Path) -> anyhow::Result<()> {
    println!("installing the envoy gateway stack");
    run(kubectl(bin).args([
        "apply",
        "--server-side",
        "-f",
        &format!(
            "https://github.com/envoyproxy/gateway/releases/download/{EG_VERSION}/install.yaml"
        ),
    ]))?;
    run(kubectl(bin).args([
        "-n",
        "envoy-gateway-system",
        "rollout",
        "status",
        "deploy/envoy-gateway",
        "--timeout=240s",
    ]))?;
    write_stdin_manifest(bin, GATEWAY_MANIFEST)?;
    run(kubectl(bin).args([
        "wait",
        "--for=condition=accepted",
        "gatewayclass/signet-eg",
        "--timeout=120s",
    ]))?;
    run(kubectl(bin).args([
        "apply",
        "-f",
        &format!("https://github.com/kubernetes-sigs/gateway-api/releases/download/{GATEWAY_API_VERSION}/standard-install.yaml"),
    ]))?;
    Ok(())
}

/// `kubectl apply -f -` from an embedded manifest; local init cannot assume
/// the repo checkout exists on the machine.
fn write_stdin_manifest(bin: &Path, manifest: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    let mut child = kubectl(bin)
        .args(["apply", "--server-side", "-f", "-"])
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
        assert!(LOCAL_STACK_MANIFEST.contains("REGISTRY_PLACEHOLDER/signet-api"));
        assert!(LOCAL_STACK_MANIFEST.contains("localhost:8081"));
    }
}
