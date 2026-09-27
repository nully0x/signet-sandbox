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
        assert_eq!(status_label(EnvStatus::Destroyed), "destroyed");
        assert_eq!(policy_label(BlockPolicy::Interval30s), "interval_30s");
        assert_eq!(policy_label(BlockPolicy::OnDemand), "on_demand");
    }
}
