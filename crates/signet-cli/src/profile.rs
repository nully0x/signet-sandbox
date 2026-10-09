// The local identity and connection profile at ~/.signet/config.toml.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use nostr::key::Keys;
use nostr::nips::nip19::ToBech32 as _;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub api_url: String,
    pub nsec: String,
    pub token: String,
}

pub fn path() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(Path::new(&home).join(".signet").join("config.toml"))
}

pub fn load() -> anyhow::Result<Profile> {
    load_from(&path()?)
}

/// Reuse the stored identity or generate one; always re-mint the token,
/// because the local database is disposable and stored tokens do not
/// survive a cluster recreate. `mint(api_url, nsec)` performs the RPC.
pub fn ensure_with<F>(path: &Path, api_url: &str, mint: F) -> anyhow::Result<Profile>
where
    F: FnOnce(&str, &str) -> anyhow::Result<String>,
{
    let existing = load_from(path).ok();
    let nsec = match &existing {
        Some(profile) => profile.nsec.clone(),
        None => Keys::generate().secret_key().to_bech32()?,
    };
    let token = mint(api_url, &nsec)?;
    let profile = Profile {
        api_url: api_url.to_string(),
        nsec,
        token,
    };
    save_to(path, &profile)?;
    Ok(profile)
}

fn load_from(path: &Path) -> anyhow::Result<Profile> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read profile {}: {e}", path.display()))?;
    toml::from_str(&raw).map_err(|e| anyhow::anyhow!("invalid profile {}: {e}", path.display()))
}

fn save_to(path: &Path, profile: &Profile) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, toml::to_string_pretty(profile)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("signet-profile-test-{tag}.toml"))
    }

    #[test]
    fn profile_round_trips_through_toml() {
        let path = temp_path("roundtrip");
        let profile = Profile {
            api_url: "http://localhost:8081".into(),
            nsec: "nsec1test".into(),
            token: "sgn_00ab".into(),
        };
        save_to(&path, &profile).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.api_url, "http://localhost:8081");
        assert_eq!(loaded.token, "sgn_00ab");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn ensure_keeps_the_identity_and_remints_the_token() {
        let path = temp_path("ensure");
        let mints = std::cell::Cell::new(0u32);
        let mint = |api: &str, key: &str| -> anyhow::Result<String> {
            mints.set(mints.get() + 1);
            assert_eq!(api, "http://localhost:8081");
            assert!(Keys::parse(key).is_ok(), "generated key must parse");
            Ok(format!("sgn_mint{}", mints.get()))
        };
        let first = ensure_with(&path, "http://localhost:8081", mint).unwrap();
        let second = ensure_with(&path, "http://localhost:8081", mint).unwrap();
        assert_eq!(first.nsec, second.nsec, "identity must be stable");
        assert_eq!(second.token, "sgn_mint2", "token re-mints on every init");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn generated_secrets_round_trip_through_keys_parse() {
        use nostr::nips::nip19::ToBech32 as _;
        let key = Keys::generate().secret_key().to_bech32().unwrap();
        assert!(Keys::parse(&key).is_ok());
    }
}
