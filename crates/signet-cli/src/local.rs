// Local mode bootstrap: one long-lived k3d cluster hosting the API and the
// environment namespaces. Mirrors `just cluster-up`.

use std::collections::{BTreeMap, BTreeSet};
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

/// k3s api port published on the server node (--api-port).
const K3S_API_PORT: u32 = 34611;
/// Fixed ports published on the server node: gateway HTTP and the local
/// provisioning api.
const SINGLE_PORTS: &[u32] = &[80, 8081];
/// Per-env window: one range, partitioned by the orchestrator (electrs,
/// explorer, lightning). Re-partition to add capacity; never append.
const ENV_PORT_WINDOW: (u32, u32) = (20000, 20399);

/// k3d releases move the cluster spec forward; a vendored copy older
/// than this gets replaced by the latest download on the next bootstrap.
const MIN_K3D_VERSION: (u32, u32) = (5, 9);

pub fn bootstrap(host: &str) -> anyhow::Result<()> {
    let bin = ensure_prerequisites()?;
    ensure_cluster(&bin)?;
    ensure_images()?;
    install_gateway(bin.as_path())?;
    deploy_local_stack(bin.as_path(), host)?;
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
    let (scheme, authority) = split_host(host);
    println!(
        "local stack ready: provisioning api on {} (endpoints on {scheme}://{authority})",
        api_url(host)
    );
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
    delete_cluster(&bin)?;
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
    ensure_k3d_version(&bin)?;
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

/// Release binaries stream over slow links; the blocking client's default
/// 30s total timeout aborts mid-body, so bound only the connect phase and
/// retry stalled reads instead.
fn download_client() -> anyhow::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(anyhow::Error::new)
}

fn fetch_with_retries<T>(
    url: &str,
    read: impl Fn(reqwest::blocking::Response) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let client = download_client()?;
    let mut last_err = None;
    for attempt in 1..=3 {
        let outcome = client
            .get(url)
            .send()
            .map_err(anyhow::Error::new)
            .and_then(|res| {
                // a bad status is a wrong URL, not a stall: fail fast
                res.error_for_status()
                    .map_err(anyhow::Error::new)
                    .and_then(&read)
            });
        match outcome {
            Ok(value) => return Ok(value),
            Err(err) => {
                if attempt < 3 {
                    eprintln!("download attempt {attempt}/3 failed ({err}); retrying in 5s");
                    std::thread::sleep(Duration::from_secs(5));
                }
                last_err = Some(err);
            }
        }
    }
    Err(last_err
        .expect("at least one download attempt")
        .context(format!("download failed after 3 attempts: {url}")))
}

fn fetch_text(url: &str) -> anyhow::Result<String> {
    fetch_with_retries(url, |res| Ok(res.text()?))
}

fn fetch_bytes(url: &str) -> anyhow::Result<Vec<u8>> {
    fetch_with_retries(url, |res| Ok(res.bytes()?.to_vec()))
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
        "darwin"
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

/// Replace a vendored k3d older than MIN_K3D_VERSION; warn on an old
/// PATH copy, which this tool does not own. Missing k3d is fine: the
/// later tool() download is latest by construction.
fn ensure_k3d_version(bin: &Path) -> anyhow::Result<()> {
    let vendored = bin.join("k3d");
    let resolved = if vendored.exists() {
        vendored.clone()
    } else if have_on_path("k3d") {
        PathBuf::from("k3d")
    } else {
        return Ok(());
    };
    let out = Command::new(&resolved).arg("version").output()?;
    let Some(version) = parse_k3d_version(&String::from_utf8_lossy(&out.stdout)) else {
        return Ok(()); // unparsable: let the create path fail loudly instead
    };
    if version >= MIN_K3D_VERSION {
        return Ok(());
    }
    if vendored.exists() {
        println!(
            "replacing vendored k3d {:?}: older than the required {:?}",
            version, MIN_K3D_VERSION
        );
        std::fs::remove_file(&vendored)?;
    } else {
        eprintln!(
            "warning: k3d on PATH is {:?}; the cluster spec needs {:?} or newer",
            version, MIN_K3D_VERSION
        );
    }
    Ok(())
}

/// (major, minor) from `k3d version` output ("k3d version v5.9.0 ...").
fn parse_k3d_version(output: &str) -> Option<(u32, u32)> {
    let line = output.lines().next()?;
    let token = line
        .split_whitespace()
        // the semver token, not neighboring words like "version"
        .find(|t| t.starts_with('v') && t[1..].starts_with(|c: char| c.is_ascii_digit()))?;
    let mut parts = token.trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
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

    if !exists {
        println!("creating k3d cluster `{CLUSTER}`");
        create_cluster(bin)?;
    } else if cluster_spec_drifted() {
        println!("cluster `{CLUSTER}` predates the current port spec; recreating");
        delete_cluster(bin)?;
        create_cluster(bin)?;
    } else {
        println!("cluster exists; starting");
        if start_cluster(bin)? {
            // k3d's failed-start rollback deletes the cluster; rebuild it.
            println!("cluster start failed and k3d deleted it; recreating");
            create_cluster(bin)?;
        }
    }

    wait_for_node(bin)?;
    Ok(())
}

/// Create the cluster with the default k3d serverlb: every `--port` is
/// a proxy-type mapping the lb owns, so the lb cannot be dropped and the
/// published count must stay well under nginx's ~1k-mapping ceiling —
/// one fixed window, never appended ranges. Create args and drift
/// detection both derive from the port constants, so they cannot
/// disagree.
fn create_cluster(bin: &Path) -> anyhow::Result<()> {
    let mut binding = k3d(bin)?;
    let cmd = binding
        .args(["cluster", "create", CLUSTER])
        .arg("--api-port")
        .arg(K3S_API_PORT.to_string());
    for port in SINGLE_PORTS {
        cmd.arg("--port").arg(format!("{port}:{port}@loadbalancer"));
    }
    let (start, end) = ENV_PORT_WINDOW;
    cmd.arg("--port")
        .arg(format!("{start}-{end}:{start}-{end}@loadbalancer"));
    cmd.arg("--k3s-arg").arg("--disable=traefik@server:0");
    cmd.arg("--volume")
        .arg("signet-data:/var/lib/rancher/k3s/storage@server:0");
    run(cmd)?;
    run(k3d(bin)?.args(["kubeconfig", "write", CLUSTER]))?;
    Ok(())
}

/// Start the existing cluster. Returns true when the start failed and
/// k3d's rollback deleted the cluster; the caller recreates in that
/// case. A start that leaves the cluster behind is a real failure and
/// surfaces as an error.
fn start_cluster(bin: &Path) -> anyhow::Result<bool> {
    let out = k3d(bin)?.args(["cluster", "start", CLUSTER]).output()?;
    if out.status.success() {
        return Ok(false);
    }
    let still_exists = k3d(bin)?
        .args(["cluster", "get", CLUSTER])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if still_exists {
        anyhow::bail!(
            "cluster start failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(true)
}

fn delete_cluster(bin: &Path) -> anyhow::Result<()> {
    run(k3d(bin)?.args(["cluster", "delete", CLUSTER]))
}

/// True when the existing cluster cannot have been created by the
/// current spec: the serverlb container owns every published port, so a
/// missing serverlb or any difference in its port bindings marks an
/// older or foreign spec. Uninspectable state counts as no drift — the
/// start path then recovers a cluster that k3d deletes on rollback.
fn cluster_spec_drifted() -> bool {
    let serverlb = format!("k3d-{CLUSTER}-serverlb");
    let out = match Command::new("docker")
        .args([
            "inspect",
            &serverlb,
            "--format",
            "{{json .HostConfig.PortBindings}}",
        ])
        .output()
    {
        Ok(out) if out.status.success() => out,
        _ => return true, // no serverlb: the current spec cannot be served
    };
    match parse_host_port_bindings(&String::from_utf8_lossy(&out.stdout)) {
        Ok(actual) => actual != expected_host_ports(),
        Err(_) => false,
    }
}

/// Host ports the local cluster must publish.
fn expected_host_ports() -> BTreeSet<u32> {
    let mut ports: BTreeSet<u32> = SINGLE_PORTS.iter().copied().collect();
    ports.insert(K3S_API_PORT);
    ports.extend(ENV_PORT_WINDOW.0..=ENV_PORT_WINDOW.1);
    ports
}

/// Host ports from `docker inspect --format '{{json .HostConfig.PortBindings}}'`.
/// Keys are container-side ("80/tcp", "20000-20399/tcp"); each binding's
/// host side is HostPort, falling back to the key's port.
fn parse_host_port_bindings(json: &str) -> anyhow::Result<BTreeSet<u32>> {
    let bindings: BTreeMap<String, serde_json::Value> = serde_json::from_str(json)?;
    let mut ports = BTreeSet::new();
    for (key, entries) in bindings {
        let Some(list) = entries.as_array() else {
            continue; // null: container port exists but is not published
        };
        if list.is_empty() {
            continue; // same meaning as null
        }
        let key_port = key.split('/').next().unwrap_or_default();
        for entry in list {
            match entry.get("HostPort").and_then(|p| p.as_str()) {
                Some(host_port) if !host_port.is_empty() => push_host_port(&mut ports, host_port)?,
                _ => push_host_port(&mut ports, key_port)?,
            }
        }
    }
    Ok(ports)
}

fn push_host_port(ports: &mut BTreeSet<u32>, value: &str) -> anyhow::Result<()> {
    let (start, end) = value.split_once('-').unwrap_or((value, value));
    let start: u32 = start
        .parse()
        .context(format!("bad port binding: {value}"))?;
    let end: u32 = end.parse().context(format!("bad port binding: {value}"))?;
    ports.extend(start..=end);
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
/// Split a `--host`/`SIGNET_ENV_HOST` value into scheme and authority.
/// Accepts a bare host (`192.168.100.2`), a URL (`http://192.168.100.2`),
/// and a trailing slash. The scheme defaults to http.
fn split_host(input: &str) -> (String, String) {
    let (scheme, authority) = input.split_once("://").unwrap_or(("http", input));
    (
        scheme.to_string(),
        authority.trim_end_matches('/').to_string(),
    )
}

/// Public URL of the api for an advertised host value.
pub fn api_url(host: &str) -> String {
    let (scheme, authority) = split_host(host);
    format!("{scheme}://{authority}:8081")
}

/// True when the advertised host is loopback, so endpoints work on this
/// machine only.
pub fn is_loopback_host(host: &str) -> bool {
    let (_, authority) = split_host(host);
    matches!(
        authority.as_str(),
        "localhost" | "127.0.0.1" | "[::1]" | "::1"
    )
}

/// Render the stack manifest for the host the api should advertise:
/// endpoint URLs and node certificates are built from this host, so
/// remote clients resolve/connect against it.
fn render_local_stack(host: &str) -> anyhow::Result<String> {
    let (scheme, authority) = split_host(host);
    Ok(LOCAL_STACK_MANIFEST
        .replace("REGISTRY_PLACEHOLDER", &registry())
        .replace("API_PUBLIC_PLACEHOLDER", &api_url(host))
        .replace("ENV_HOST_PLACEHOLDER", &format!("{scheme}://{authority}")))
}

fn deploy_local_stack(bin: &Path, host: &str) -> anyhow::Result<()> {
    println!("deploying postgres + provisioning api");
    let manifest = render_local_stack(host)?;
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

/// The k3d serverlb maps host 8081 through to the api's LoadBalancer
/// service; poll until the routing chain answers.
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
fn install_gateway(bin: &Path) -> anyhow::Result<()> {
    println!("installing the envoy gateway stack");
    // Apply the installer with its CRDs: k3s does not ship the gateway-api
    // CRDs, and without them the controller crash-loops on "no matches for
    // kind Gateway" while the rollout waits forever. Server-side apply
    // reconciles them if a future k3s starts shipping its own copies.
    let eg_install = fetch_text(&format!(
        "https://github.com/envoyproxy/gateway/releases/download/{EG_VERSION}/install.yaml"
    ))?;
    write_stdin_manifest(bin, &eg_install)?;
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
        let os = os_name();
        let url = k3d_download_url();
        assert!(url.starts_with("https://github.com/k3d-io/k3d/releases/latest/download/k3d-"));
        assert!(url.ends_with(&format!("-{os}-amd64")) || url.ends_with(&format!("-{os}-arm64")));
        let kubectl = kubectl_download_url();
        assert!(kubectl.contains("{version}"));
        assert!(
            kubectl.contains(&format!("/bin/{os}/amd64/"))
                || kubectl.contains(&format!("/bin/{os}/arm64/"))
        );
    }

    #[test]
    fn expected_ports_cover_fixed_and_window() {
        let ports = expected_host_ports();
        assert_eq!(ports.len(), 403);
        for port in [80, 8081, 34611, 20000, 20399] {
            assert!(ports.contains(&port), "missing {port}");
        }
        assert!(!ports.contains(&443));
    }

    #[test]
    fn port_bindings_parse_singles_ranges_and_nulls() {
        let json = r#"{
            "80/tcp": [{"HostIp": "0.0.0.0", "HostPort": "80"}],
            "8081/tcp": [{"HostIp": "", "HostPort": "8081"}],
            "20000/tcp": [{"HostIp": "0.0.0.0", "HostPort": "20000"}],
            "20001-20003/tcp": [{"HostIp": "0.0.0.0", "HostPort": "20001-20003"}],
            "20399/tcp": [],
            "443/tcp": null,
            "9000/tcp": [{"HostIp": "", "HostPort": ""}]
        }"#;
        let ports = parse_host_port_bindings(json).unwrap();
        assert!(ports.contains(&80));
        assert!(ports.contains(&8081));
        assert!(ports.contains(&20000));
        assert!(ports.contains(&20003));
        assert!(ports.contains(&9000));
        // empty array and null both mean "not published"
        assert!(!ports.contains(&20399));
        assert!(!ports.contains(&443));
        assert_eq!(ports.intersection(&expected_host_ports()).count(), 6);
    }

    #[test]
    fn port_bindings_reject_garbage() {
        assert!(parse_host_port_bindings("not json").is_err());
        assert!(parse_host_port_bindings(r#"{"x/tcp": [{"HostPort": "abc"}]}"#).is_err());
    }

    #[test]
    fn k3d_version_parses_from_banner() {
        let out = "k3d version v5.9.0\nk3s version v1.32.5-k3s1 (default)";
        assert_eq!(parse_k3d_version(out), Some((5, 9)));
        assert_eq!(parse_k3d_version("k3d version devel"), None);
        assert_eq!(parse_k3d_version(""), None);
    }

    #[test]
    fn host_values_normalize_to_scheme_and_authority() {
        assert_eq!(
            split_host("localhost"),
            ("http".to_string(), "localhost".to_string())
        );
        assert_eq!(
            split_host("192.168.100.2"),
            ("http".to_string(), "192.168.100.2".to_string())
        );
        assert_eq!(
            split_host("http://192.168.100.2/"),
            ("http".to_string(), "192.168.100.2".to_string())
        );
        assert_eq!(
            split_host("https://sandbox.example.com"),
            ("https".to_string(), "sandbox.example.com".to_string())
        );
    }

    #[test]
    fn rendered_stack_advertises_the_host_once() {
        let manifest = render_local_stack("http://192.168.100.2/").unwrap();
        assert!(manifest.contains("value: http://192.168.100.2:8081"));
        assert!(manifest.contains("value: http://192.168.100.2 }"));
        assert!(!manifest.contains("http://http://"));
        assert!(!manifest.contains("PLACEHOLDER"));
    }

    #[test]
    fn loopback_hosts_are_recognised() {
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("http://localhost/"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(!is_loopback_host("192.168.100.2"));
        assert!(!is_loopback_host("https://sandbox.example.com"));
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
