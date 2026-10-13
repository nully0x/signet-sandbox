// The platform version catalog: which component versions users can pin,
// which is the default per component, and how versions constrain each
// other. Single source of truth for api validation, the catalog.list
// surface, and the CI publish matrix.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionEntry {
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub requires: BTreeMap<String, String>,
}

/// component -> version -> entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionCatalog {
    #[serde(flatten)]
    entries: BTreeMap<String, BTreeMap<String, VersionEntry>>,
}

/// Embedded copy of the repo-level catalog.
pub const VERSIONS_TOML: &str = include_str!("../../../versions.toml");

impl VersionCatalog {
    pub fn parse(toml: &str) -> Result<Self, String> {
        toml::from_str(toml).map_err(|e| format!("invalid version catalog: {e}"))
    }

    /// The embedded repo catalog, parsed.
    pub fn embedded() -> Self {
        Self::parse(VERSIONS_TOML).expect("embedded version catalog is valid")
    }

    pub fn entries(&self) -> &BTreeMap<String, BTreeMap<String, VersionEntry>> {
        &self.entries
    }

    pub fn default_version(&self, component: &str) -> Option<&str> {
        self.entries
            .get(component)?
            .iter()
            .find(|(_, entry)| entry.default)
            .map(|(version, _)| version.as_str())
    }

    pub fn has(&self, component: &str, version: &str) -> bool {
        self.entries
            .get(component)
            .is_some_and(|versions| versions.contains_key(version))
    }

    /// Validate a user selection: every pinned version must exist in the
    /// catalog, and every entry's `requires` constraints must hold against
    /// the effective selection (pinned versions, defaults elsewhere).
    /// The default bitcoind participates in constraints even when the user
    /// did not pin it.
    pub fn validate_selection(&self, tags: &BTreeMap<String, String>) -> Result<(), String> {
        for (component, version) in tags {
            if !self.has(component, version) {
                let available = self
                    .entries
                    .get(component)
                    .map(|versions| versions.keys().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_else(|| "none".to_string());
                return Err(format!(
                    "unknown version `{version}` for `{component}` (available: {available})"
                ));
            }
        }

        for (component, versions) in &self.entries {
            let selected = match tags.get(component) {
                Some(v) => Some(v.clone()),
                None => self.default_version(component).map(str::to_string),
            };
            let Some(version) = selected else { continue };
            let Some(entry) = versions.get(&version) else {
                continue;
            };
            for (dependency, constraint) in &entry.requires {
                let dep_version = match tags.get(dependency) {
                    Some(v) => v.clone(),
                    None => match self.default_version(dependency) {
                        Some(v) => v.to_string(),
                        None => continue,
                    },
                };
                if !satisfies(&dep_version, constraint) {
                    return Err(format!(
                        "{component} {version} requires {dependency} {constraint}, but {dep_version} was selected"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Split a version string into numeric segments; non-numeric suffixes
/// (`-beta`, a leading `v`) are ignored, missing segments count as zero.
fn version_segments(version: &str) -> Vec<u64> {
    version
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn compare_segments(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let len = a.len().max(b.len());
    for i in 0..len {
        let left = a.get(i).copied().unwrap_or(0);
        let right = b.get(i).copied().unwrap_or(0);
        match left.cmp(&right) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Evaluate a constraint like `>= 25`, `< 29`, `= 0.19.2-beta` against a
/// version string.
pub fn satisfies(version: &str, constraint: &str) -> bool {
    let constraint = constraint.trim();
    let (operator, target) = if let Some(rest) = constraint.strip_prefix(">=") {
        (">=", rest)
    } else if let Some(rest) = constraint.strip_prefix("<=") {
        ("<=", rest)
    } else if let Some(rest) = constraint.strip_prefix('>') {
        (">", rest)
    } else if let Some(rest) = constraint.strip_prefix('<') {
        ("<", rest)
    } else if let Some(rest) = constraint.strip_prefix('=') {
        ("=", rest)
    } else {
        ("=", constraint)
    };
    let order = compare_segments(&version_segments(version), &version_segments(target.trim()));
    match operator {
        ">=" => order != std::cmp::Ordering::Less,
        "<=" => order != std::cmp::Ordering::Greater,
        ">" => order == std::cmp::Ordering::Greater,
        "<" => order == std::cmp::Ordering::Less,
        _ => order == std::cmp::Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_catalog_shape() {
        let catalog = VersionCatalog::parse(
            r#"
[bitcoind."29.4"]
default = true

[electrs."0.11.1"]
default = true

[electrs."0.10.9"]
requires = { bitcoind = "< 29" }
"#,
        )
        .unwrap();
        assert_eq!(catalog.default_version("bitcoind"), Some("29.4"));
        assert!(catalog.has("electrs", "0.10.9"));
        assert!(!catalog.has("electrs", "9.9"));
        assert_eq!(
            catalog.entries()["electrs"]["0.10.9"].requires["bitcoind"],
            "< 29"
        );
    }

    #[test]
    fn range_constraints_compare_numerically() {
        assert!(satisfies("29.4", ">= 25"));
        assert!(!satisfies("28.4", ">= 29"));
        assert!(satisfies("0.10.9", "< 29"));
        assert!(satisfies("0.19.2-beta", ">= 0.19"));
        assert!(!satisfies("0.18.5-beta", ">= 0.19"));
        assert!(satisfies("v3.3.1", ">= 3"));
        assert!(satisfies("0.19.2-beta", "= 0.19.2-beta"));
        assert!(!satisfies("0.18.5-beta", "= 0.19.2-beta"));
    }

    #[test]
    fn validate_rejects_unknown_versions() {
        let catalog = VersionCatalog::embedded();
        let err = catalog
            .validate_selection(&BTreeMap::from([(
                "electrs".to_string(),
                "9.9".to_string(),
            )]))
            .unwrap_err();
        assert!(err.contains("unknown version"), "{err}");
    }

    #[test]
    fn validate_rejects_incompatible_combinations() {
        // A synthetic catalog: the mechanism is enforced hard when a real
        // incompatibility is recorded in versions.toml.
        let catalog = VersionCatalog::parse(
            r#"
[bitcoind."29.4"]
default = true

[electrs."0.10.9"]
requires = { bitcoind = "< 29" }
"#,
        )
        .unwrap();
        let err = catalog
            .validate_selection(&BTreeMap::from([(
                "electrs".to_string(),
                "0.10.9".to_string(),
            )]))
            .unwrap_err();
        assert!(err.contains("requires bitcoind"), "{err}");
    }

    #[test]
    fn validate_passes_defaults_and_explicit_pins() {
        let catalog = VersionCatalog::embedded();
        catalog.validate_selection(&BTreeMap::new()).unwrap();
        catalog
            .validate_selection(&BTreeMap::from([
                ("electrs".to_string(), "0.11.1".to_string()),
                ("lnd".to_string(), "0.19.2-beta".to_string()),
            ]))
            .unwrap();
    }
}
