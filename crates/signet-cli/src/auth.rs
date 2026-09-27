// Client credentials: API bearer token or per-request NIP-98 signing.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use nostr::event::{EventBuilder, FinalizeEvent, Kind, Tag};
use nostr::key::Keys;
use nostr::types::Timestamp;

pub enum Auth {
    Bearer(String),
    Nip98(Keys),
}

pub fn from_flags(token: Option<String>, nsec: Option<String>) -> anyhow::Result<Auth> {
    match (token, nsec) {
        (Some(_), Some(_)) => {
            anyhow::bail!("use --token or --nsec, not both")
        }
        (Some(token), None) => Ok(Auth::Bearer(token)),
        (None, Some(key)) => {
            Ok(Auth::Nip98(Keys::parse(&key).map_err(|e| {
                anyhow::anyhow!("invalid Nostr secret key: {e}")
            })?))
        }
        (None, None) => anyhow::bail!(
            "no credentials: pass --token or --nsec, or set SIGNET_TOKEN / SIGNET_NSEC"
        ),
    }
}

impl Auth {
    /// Build the `Authorization` header for one request.
    /// NIP-98 events are minted fresh per call: the signature covers the URL.
    pub fn header(&self, url: &str, method: &str) -> anyhow::Result<String> {
        match self {
            Auth::Bearer(token) => Ok(format!("Bearer {token}")),
            Auth::Nip98(keys) => {
                let event = EventBuilder::new(Kind::HttpAuth, "")
                    .tags(vec![
                        Tag::custom("u", [url]),
                        Tag::custom("method", [method]),
                    ])
                    .custom_created_at(Timestamp::now())
                    .finalize(keys)?;
                Ok(format!(
                    "{} {}",
                    signet_nostr::NIP98_SCHEME,
                    BASE64.encode(serde_json::to_vec(&event)?)
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use signet_nostr::verify_nip98_header;
    use std::time::Duration;

    fn test_key() -> String {
        "0000000000000000000000000000000000000000000000000000000000000001".to_string()
    }

    #[test]
    fn bearer_header_passthrough() {
        let auth = from_flags(Some("sgn_00ab".into()), None).unwrap();
        assert_eq!(
            auth.header("http://x/v1/rpc", "POST").unwrap(),
            "Bearer sgn_00ab"
        );
    }

    #[test]
    fn nip98_header_verifies_against_the_server() {
        let auth = from_flags(None, Some(test_key())).unwrap();
        let header = auth.header("http://localhost:8081/v1/rpc", "POST").unwrap();
        let npub = verify_nip98_header(
            &header,
            "http://localhost:8081/v1/rpc",
            "POST",
            Some(Duration::from_secs(300)),
        )
        .unwrap();
        assert!(npub.starts_with("npub1"));
    }

    #[test]
    fn nip98_header_rejects_a_different_url() {
        let auth = from_flags(None, Some(test_key())).unwrap();
        let header = auth.header("http://localhost:8081/v1/rpc", "POST").unwrap();
        assert!(
            verify_nip98_header(
                &header,
                "http://elsewhere/v1/rpc",
                "POST",
                Some(Duration::from_secs(300)),
            )
            .is_err()
        );
    }

    #[test]
    fn flags_require_exactly_one_credential() {
        assert!(from_flags(None, None).is_err());
        assert!(from_flags(Some("sgn_x".into()), Some(test_key())).is_err());
    }
}
