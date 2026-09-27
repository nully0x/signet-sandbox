// Terminal rendering: human key-value output, or raw JSON with --json.

use serde::Serialize;
use signet_core::{BlockPolicy, ConnectionBundle, EnvStatus};

pub fn status_label(status: EnvStatus) -> &'static str {
    match status {
        EnvStatus::Provisioning => "provisioning",
        EnvStatus::Ready => "ready",
        EnvStatus::Stopped => "stopped",
        EnvStatus::Expired => "expired",
        EnvStatus::Destroyed => "destroyed",
    }
}

fn policy_label(policy: BlockPolicy) -> &'static str {
    match policy {
        BlockPolicy::Interval30s => "interval_30s",
        BlockPolicy::OnDemand => "on_demand",
    }
}

pub fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// One row of `environment.list` (subset of the environment record).
#[derive(Debug, serde::Deserialize)]
pub struct ListRow {
    pub id: String,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

pub fn render_list(rows: &[ListRow]) -> String {
    if rows.is_empty() {
        return "no environments\n".to_string();
    }
    let name_w = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let status_w = rows
        .iter()
        .map(|r| r.status.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let mut out = String::new();
    out.push_str(&format!(
        "{:<name_w$}  {:<38}  {:<status_w$}  {}\n",
        "NAME", "ID", "STATUS", "EXPIRES_AT"
    ));
    for row in rows {
        out.push_str(&format!(
            "{:<name_w$}  {:<38}  {:<status_w$}  {}\n",
            row.name,
            row.id,
            row.status,
            row.expires_at.as_deref().unwrap_or("-"),
        ));
    }
    out
}

pub fn print_bundle_human(bundle: &ConnectionBundle) {
    println!("environment_id:   {}", bundle.environment_id);
    println!("status:           {}", status_label(bundle.status));
    println!("rpc_url:          {}", bundle.rpc_url);
    if let Some(auth) = &bundle.rpc_auth {
        println!("rpc_auth:         {auth}");
    }
    if let Some(endpoint) = &bundle.indexer_endpoint {
        println!("indexer_endpoint: {endpoint}");
    }
    if let Some(url) = &bundle.explorer_url {
        println!("explorer_url:     {url}");
    }
    if let Some(url) = &bundle.faucet_url {
        println!("faucet_url:       {url}");
    }
    if let Some(lightning) = &bundle.lightning {
        println!("lightning_rest:   {}", lightning.rest_url);
    }
    println!("signet_challenge: {}", bundle.signet_challenge);
    println!("block_policy:     {}", policy_label(bundle.block_policy));
    if let Some(versions) = &bundle.versions {
        for (component, image) in versions {
            println!("version {component}:  {image}");
        }
    }
    if let Some(expires_at) = &bundle.expires_at {
        println!("expires_at:       {expires_at}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_policy_labels_match_the_wire_names() {
        assert_eq!(status_label(EnvStatus::Provisioning), "provisioning");
        assert_eq!(status_label(EnvStatus::Ready), "ready");
        assert_eq!(status_label(EnvStatus::Stopped), "stopped");
        assert_eq!(status_label(EnvStatus::Destroyed), "destroyed");
        assert_eq!(policy_label(BlockPolicy::Interval30s), "interval_30s");
        assert_eq!(policy_label(BlockPolicy::OnDemand), "on_demand");
    }

    fn row(name: &str, status: &str, expires: Option<&str>) -> ListRow {
        ListRow {
            id: "0195c7c5-0000-7000-8000-000000000000".into(),
            name: name.into(),
            status: status.into(),
            expires_at: expires.map(Into::into),
        }
    }

    #[test]
    fn list_renders_aligned_columns() {
        let rows = vec![
            row("acme-staging", "ready", Some("2026-09-28T00:00:00Z")),
            row("quick", "stopped", None),
        ];
        let table = render_list(&rows);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("NAME"));
        assert!(lines[0].contains("EXPIRES_AT"));
        assert!(lines[1].contains("acme-staging"));
        assert!(lines[1].contains("2026-09-28T00:00:00Z"));
        assert!(lines[2].contains("stopped"));
        assert!(lines[2].ends_with('-'));
    }

    #[test]
    fn empty_list_renders_a_hint() {
        assert_eq!(render_list(&[]), "no environments\n");
    }

    #[test]
    fn list_rows_parse_the_wire_shape() {
        let parsed: Vec<ListRow> = serde_json::from_value(serde_json::json!([
            {
                "id": "0195c7c5-0000-7000-8000-000000000000",
                "name": "n",
                "status": "ready",
                "created_at": "2026-09-27T00:00:00Z",
                "expires_at": null
            }
        ]))
        .unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].status, "ready");
        assert!(parsed[0].expires_at.is_none());
    }
}
