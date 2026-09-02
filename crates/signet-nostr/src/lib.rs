use std::fmt;
use std::time::Duration;

use bitcoin::hashes::{Hash as _, sha256};
use bitcoin::hex::{DisplayHex as _, FromHex as _};
use bitcoin::secp256k1::rand::{RngCore, rngs::OsRng};
use nostr::event::{Event, Kind};
use nostr::key::PublicKey;
use nostr::types::Timestamp;
use thiserror::Error;

pub const NIP98_KIND: Kind = Kind::HttpAuth;

pub const TOKEN_PREFIX: &str = "sgn";

const TOKEN_SECRET_BYTES: usize = 32;

const MAX_FUTURE_SKEW: Duration = Duration::from_secs(60);

#[derive(Debug, Error)]
pub enum Nip98Error {
    #[error("event id or signature verification failed")]
    BadEvent(#[from] nostr::error::Error),
    #[error("event is not a NIP-98 http auth event")]
    WrongKind,
    #[error("missing `{0}` tag")]
    MissingTag(&'static str),
    #[error("`u` tag does not match the request URL")]
    UrlMismatch,
    #[error("`method` tag does not match the request method")]
    MethodMismatch,
    #[error("`created_at` outside the acceptance window")]
    Stale,
}

#[derive(Debug, Error)]
#[error("token must be `{TOKEN_PREFIX}_` followed by 64 hex characters")]
pub struct MalformedToken;

pub struct ApiToken {
    raw: String,
}

impl ApiToken {
    pub fn generate() -> Self {
        let mut secret = [0u8; TOKEN_SECRET_BYTES];
        OsRng.fill_bytes(&mut secret);
        Self {
            raw: format!("{TOKEN_PREFIX}_{}", secret.as_hex()),
        }
    }

    pub fn parse(raw: &str) -> Result<Self, MalformedToken> {
        let hex = raw
            .strip_prefix(TOKEN_PREFIX)
            .and_then(|rest| rest.strip_prefix('_'))
            .ok_or(MalformedToken)?;
        let secret = <[u8; TOKEN_SECRET_BYTES]>::from_hex(hex).map_err(|_| MalformedToken)?;
        Ok(Self {
            raw: format!("{TOKEN_PREFIX}_{}", secret.as_hex()),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn hash(&self) -> TokenHash {
        TokenHash::from_raw(&self.raw)
    }
}

impl fmt::Display for ApiToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

pub struct TokenHash {
    hex: String,
}

impl TokenHash {
    pub fn from_raw(raw: &str) -> Self {
        Self {
            hex: sha256::Hash::hash(raw.as_bytes()).to_string(),
        }
    }

    pub fn as_hex(&self) -> &str {
        &self.hex
    }

    pub fn matches(&self, other: &TokenHash) -> bool {
        constant_time_eq(self.hex.as_bytes(), other.hex.as_bytes())
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn verify_http_auth(
    event: &Event,
    url: &str,
    method: &str,
    now: Timestamp,
    max_age: Duration,
) -> Result<PublicKey, Nip98Error> {
    event.verify()?;

    if event.kind != NIP98_KIND {
        return Err(Nip98Error::WrongKind);
    }

    if event.created_at > now + MAX_FUTURE_SKEW
        || now - event.created_at > Timestamp::from_secs(max_age.as_secs())
    {
        return Err(Nip98Error::Stale);
    }

    match tag_value(event, "u") {
        Some(u) if u == url => {}
        Some(_) => return Err(Nip98Error::UrlMismatch),
        None => return Err(Nip98Error::MissingTag("u")),
    }

    match tag_value(event, "method") {
        Some(m) if m.eq_ignore_ascii_case(method) => {}
        Some(_) => return Err(Nip98Error::MethodMismatch),
        None => return Err(Nip98Error::MissingTag("method")),
    }

    Ok(event.pubkey)
}

pub fn payload_hash(event: &Event) -> Option<&str> {
    tag_value(event, "payload")
}

fn tag_value<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    event.tags.iter().find_map(|tag| {
        let values = tag.as_slice();
        if values.first().map(String::as_str) == Some(name) {
            values.get(1).map(String::as_str)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::event::{EventBuilder, FinalizeEvent, Tag};
    use nostr::key::Keys;

    const URL: &str = "http://localhost:8080/rpc";
    const MAX_AGE: Duration = Duration::from_secs(300);

    fn keys(n: u8) -> Keys {
        Keys::parse(&format!("{n:064x}")).unwrap()
    }

    fn signed(keys: &Keys, created_at: Timestamp, extra_tags: Vec<Tag>) -> Event {
        EventBuilder::new(NIP98_KIND, "")
            .tags(extra_tags)
            .custom_created_at(created_at)
            .finalize(keys)
            .unwrap()
    }

    fn auth_event(keys: &Keys) -> Event {
        signed(
            keys,
            Timestamp::now(),
            vec![Tag::custom("u", [URL]), Tag::custom("method", ["POST"])],
        )
    }

    fn verify(event: &Event) -> Result<PublicKey, Nip98Error> {
        verify_http_auth(event, URL, "POST", Timestamp::now(), MAX_AGE)
    }

    #[test]
    fn valid_event_returns_signer_pubkey() {
        let keys = keys(1);
        let pubkey = verify(&auth_event(&keys)).unwrap();
        assert_eq!(pubkey, keys.public_key());
    }

    #[test]
    fn stale_event_rejected() {
        let keys = keys(1);
        let event = signed(
            &keys,
            Timestamp::now() - Duration::from_secs(301),
            vec![Tag::custom("u", [URL]), Tag::custom("method", ["POST"])],
        );
        assert!(matches!(verify(&event), Err(Nip98Error::Stale)));
    }

    #[test]
    fn future_event_rejected() {
        let keys = keys(1);
        let event = signed(
            &keys,
            Timestamp::now() + Duration::from_secs(120),
            vec![Tag::custom("u", [URL]), Tag::custom("method", ["POST"])],
        );
        assert!(matches!(verify(&event), Err(Nip98Error::Stale)));
    }

    #[test]
    fn wrong_kind_rejected() {
        let keys = keys(1);
        let event = EventBuilder::new(Kind::TextNote, "")
            .tags(vec![
                Tag::custom("u", [URL]),
                Tag::custom("method", ["POST"]),
            ])
            .custom_created_at(Timestamp::now())
            .finalize(&keys)
            .unwrap();
        assert!(matches!(verify(&event), Err(Nip98Error::WrongKind)));
    }

    #[test]
    fn missing_url_tag_rejected() {
        let keys = keys(1);
        let event = signed(
            &keys,
            Timestamp::now(),
            vec![Tag::custom("method", ["POST"])],
        );
        assert!(matches!(verify(&event), Err(Nip98Error::MissingTag("u"))));
    }

    #[test]
    fn url_mismatch_rejected() {
        let keys = keys(1);
        let event = signed(
            &keys,
            Timestamp::now(),
            vec![
                Tag::custom("u", ["http://evil.example/rpc"]),
                Tag::custom("method", ["POST"]),
            ],
        );
        assert!(matches!(verify(&event), Err(Nip98Error::UrlMismatch)));
    }

    #[test]
    fn method_mismatch_rejected() {
        let keys = keys(1);
        let event = signed(
            &keys,
            Timestamp::now(),
            vec![Tag::custom("u", [URL]), Tag::custom("method", ["GET"])],
        );
        assert!(matches!(verify(&event), Err(Nip98Error::MethodMismatch)));
    }

    #[test]
    fn tampered_content_rejected() {
        let keys = keys(1);
        let event = auth_event(&keys);
        let tampered = Event::new(
            event.id,
            event.pubkey,
            event.created_at,
            event.kind,
            event.tags.to_vec(),
            "tampered",
            event.sig,
        );
        assert!(matches!(verify(&tampered), Err(Nip98Error::BadEvent(_))));
    }

    #[test]
    fn payload_hash_exposed() {
        let keys = keys(1);
        let event = signed(
            &keys,
            Timestamp::now(),
            vec![
                Tag::custom("u", [URL]),
                Tag::custom("method", ["POST"]),
                Tag::custom("payload", ["0123abcd"]),
            ],
        );
        assert_eq!(payload_hash(&event), Some("0123abcd"));
        assert_eq!(payload_hash(&auth_event(&keys)), None);
    }

    #[test]
    fn generated_token_has_expected_shape() {
        let token = ApiToken::generate();
        let raw = token.as_str();
        let hex = raw.strip_prefix("sgn_").unwrap();
        assert_eq!(hex.len(), 64);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn generated_tokens_are_unique() {
        assert_ne!(ApiToken::generate().as_str(), ApiToken::generate().as_str());
    }

    #[test]
    fn parse_round_trips() {
        let token = ApiToken::generate();
        let reparsed = ApiToken::parse(token.as_str()).unwrap();
        assert_eq!(reparsed.as_str(), token.as_str());
        assert!(reparsed.hash().matches(&token.hash()));
    }

    #[test]
    fn parse_rejects_malformed_tokens() {
        for raw in [
            "",
            "sgn",
            "sgn_",
            "sgn_0",
            "sgn_00",
            "not_x64hexcharsandthenmorehexcharactersuntilweareat64characters00",
            "sgn_zzzz0000000000000000000000000000000000000000000000000000000000",
            "other_0000000000000000000000000000000000000000000000000000000000000000",
        ] {
            assert!(ApiToken::parse(raw).is_err(), "accepted {raw:?}");
        }
    }

    #[test]
    fn hash_is_deterministic_per_token() {
        let token = ApiToken::generate();
        assert_eq!(token.hash().as_hex(), token.hash().as_hex());
        assert_ne!(token.hash().as_hex(), ApiToken::generate().hash().as_hex());
    }

    #[test]
    fn hash_matches_presented_token_against_stored() {
        let issued = ApiToken::generate();
        let stored = issued.hash();

        let presented = ApiToken::parse(issued.as_str()).unwrap();
        assert!(TokenHash::from_raw(presented.as_str()).matches(&stored));

        let wrong = TokenHash::from_raw(ApiToken::generate().as_str());
        assert!(!wrong.matches(&stored));
    }
}
