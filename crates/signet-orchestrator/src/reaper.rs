// Reaper: deletes environment namespaces whose expires-at has passed (spec §13).

use chrono::{DateTime, Utc};
use k8s_openapi::api::core::v1::Namespace;
use kube::Error;
use kube::api::{Api, DeleteParams, ListParams};
use kube::core::DynamicObject;

use crate::{
    ENV_LABEL, EXPIRES_AT_ANNOTATION, GATEWAY_NAMESPACE, OrchestrateError, Orchestrator,
    reference_grant_api,
};

impl Orchestrator {
    /// Deletes every expired environment namespace and returns their names.
    /// One failed delete must not abort the pass.
    pub async fn reap_expired(&self) -> Result<Vec<String>, OrchestrateError> {
        let nss: Vec<Namespace> = Api::<Namespace>::all(self.client.clone())
            .list(&ListParams::default().labels(ENV_LABEL))
            .await?
            .items;
        let mut reaped = Vec::new();
        for name in expired_namespace_names(Utc::now(), &nss) {
            match self.delete_env_namespace(&name).await {
                Ok(()) => {
                    tracing::info!(namespace = %name, "reaped expired environment namespace");
                    reaped.push(name);
                }
                Err(e) => tracing::warn!(namespace = %name, error = %e, "reap delete failed"),
            }
        }
        Ok(reaped)
    }

    pub(crate) async fn delete_env_namespace(&self, ns: &str) -> Result<(), OrchestrateError> {
        match Api::<Namespace>::all(self.client.clone())
            .delete(ns, &DeleteParams::default())
            .await
        {
            Ok(_) => {}
            // Already gone: a concurrent destroy or reaper pass won the race.
            Err(Error::Api(status)) if status.code == 404 => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        // The ReferenceGrant lives in the Gateway namespace and is best-effort:
        // a grant naming a deleted namespace authorizes nothing, so a failed
        // cleanup must not mask the deletion.
        let grants = Api::<DynamicObject>::namespaced_with(
            self.client.clone(),
            GATEWAY_NAMESPACE,
            &reference_grant_api(),
        );
        if let Err(e) = grants.delete(ns, &DeleteParams::default()).await
            && !matches!(&e, Error::Api(status) if status.code == 404)
        {
            tracing::warn!(error = %e, env_ns = %ns, "ReferenceGrant cleanup failed");
        }
        Ok(())
    }
}

/// Namespaces whose `expires-at` annotation is past `now`. Without the env
/// label or with an unparsable timestamp a namespace is never selected — the
/// reaper only deletes what it can prove expired.
pub(crate) fn expired_namespace_names(now: DateTime<Utc>, namespaces: &[Namespace]) -> Vec<String> {
    namespaces
        .iter()
        .filter(|ns| {
            let labelled = ns
                .metadata
                .labels
                .as_ref()
                .is_some_and(|l| l.contains_key(ENV_LABEL));
            let expired = ns
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(EXPIRES_AT_ANNOTATION))
                .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
                .is_some_and(|t| t < now);
            labelled && expired
        })
        .filter_map(|ns| ns.metadata.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace_object;
    use std::collections::BTreeMap;

    #[test]
    fn past_due_namespace_is_reaped() {
        let expired = namespace_object("aaa111", Some(Utc::now() - chrono::Duration::seconds(60)));
        let live = namespace_object("bbb222", Some(Utc::now() + chrono::Duration::seconds(3600)));
        assert_eq!(
            expired_namespace_names(Utc::now(), &[expired, live]),
            vec!["env-aaa111".to_string()]
        );
    }

    #[test]
    fn unprovable_namespaces_are_never_reaped() {
        let no_annotation = namespace_object("aaa111", None);
        let mut garbage = namespace_object("bbb222", None);
        garbage.metadata.annotations = Some(BTreeMap::from([(
            EXPIRES_AT_ANNOTATION.to_string(),
            "not-a-timestamp".to_string(),
        )]));
        let mut unlabelled =
            namespace_object("ccc333", Some(Utc::now() - chrono::Duration::seconds(60)));
        unlabelled.metadata.labels = None;
        assert!(
            expired_namespace_names(Utc::now(), &[no_annotation, garbage, unlabelled]).is_empty()
        );
    }
}
