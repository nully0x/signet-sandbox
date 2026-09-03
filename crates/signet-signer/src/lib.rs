use bitcoin::secp256k1::{Secp256k1, SecretKey};
use bitcoin::{Network, PrivateKey};
use serde::Serialize;

pub mod signet;

#[derive(Debug, Clone, Serialize)]
pub struct SignerKey {
    pub wif: String,
    pub pubkey: String,
    pub challenge: String,
}

pub fn generate_key() -> SignerKey {
    let secp = Secp256k1::new();
    let key = SecretKey::new(&mut bitcoin::secp256k1::rand::thread_rng());
    let pubkey = key.public_key(&secp);
    let challenge = signet::challenge_for_pubkey(&pubkey);
    let wif = PrivateKey::new(key, Network::Testnet).to_wif();
    SignerKey {
        wif,
        pubkey: pubkey.to_string(),
        challenge: challenge.to_hex_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_has_consistent_challenge() {
        let key = generate_key();
        assert!(key.wif.starts_with('5') || key.wif.starts_with('9') || key.wif.starts_with('c'));
        assert_eq!(key.pubkey.len(), 66);
        assert!(key.challenge.starts_with("5121"));
        assert!(key.challenge.ends_with("51ae"));
    }
}
