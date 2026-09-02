use bitcoin::absolute::LockTime;
use bitcoin::blockdata::block::{Header, Version as BlockVersion};
use bitcoin::blockdata::script::{Builder, PushBytesBuf};
use bitcoin::blockdata::transaction::{
    OutPoint, Sequence, Transaction, TxIn, TxOut, Version as TxVersion,
};
use bitcoin::consensus::encode::deserialize_hex;
use bitcoin::hashes::{Hash, sha256d};
use bitcoin::opcodes::all::{OP_CHECKMULTISIG, OP_PUSHNUM_1, OP_RETURN};
use bitcoin::secp256k1::{Message, Secp256k1, SecretKey, ecdsa::Signature};
use bitcoin::sighash::SighashCache;
use bitcoin::{
    Amount, Block, BlockHash, CompactTarget, Script, ScriptBuf, Target, TxMerkleNode, Witness,
};
use thiserror::Error;

use signet_bitcoind::BlockTemplate;

pub const SIGNET_HEADER: [u8; 4] = [0xec, 0xc7, 0xda, 0xa2];
pub const WITNESS_COMMITMENT_PREFIX: [u8; 4] = [0xaa, 0x21, 0xa9, 0xed];

#[derive(Debug, Error)]
pub enum SignetError {
    #[error("transaction decode: {0}")]
    Decode(String),
    #[error("challenge mismatch: template {template} != configured {configured}")]
    ChallengeMismatch {
        template: String,
        configured: String,
    },
    #[error("sighash computation failed")]
    Sighash,
    #[error("no valid nonce found")]
    NoNonce,
}

pub fn challenge_for_pubkey(pubkey: &bitcoin::secp256k1::PublicKey) -> ScriptBuf {
    Builder::new()
        .push_opcode(OP_PUSHNUM_1)
        .push_slice(pubkey.serialize())
        .push_opcode(OP_PUSHNUM_1)
        .push_opcode(OP_CHECKMULTISIG)
        .into_script()
}

fn append_push(script: ScriptBuf, data: &[u8]) -> ScriptBuf {
    let mut bytes = script.into_bytes();
    match data.len() {
        0..=0x4b => bytes.push(data.len() as u8),
        0x4c..=0xff => {
            bytes.push(0x4c);
            bytes.push(data.len() as u8);
        }
        _ => {
            bytes.push(0x4d);
            bytes.extend_from_slice(&(data.len() as u16).to_le_bytes());
        }
    }
    bytes.extend_from_slice(data);
    ScriptBuf::from_bytes(bytes)
}

pub fn merkle_root(mut leaves: Vec<[u8; 32]>) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    while leaves.len() > 1 {
        if leaves.len() % 2 == 1 {
            let last = *leaves.last().expect("non-empty");
            leaves.push(last);
        }
        leaves = leaves
            .chunks(2)
            .map(|pair| {
                let mut data = [0u8; 64];
                data[..32].copy_from_slice(&pair[0]);
                data[32..].copy_from_slice(&pair[1]);
                sha256d::Hash::hash(&data).to_byte_array()
            })
            .collect();
    }
    leaves[0]
}

fn push_bytes(v: Vec<u8>) -> PushBytesBuf {
    PushBytesBuf::try_from(v).expect("push within consensus limits")
}

fn compact_size(len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    match len {
        0..=0xfc => out.push(len as u8),
        0xfd..=0xffff => {
            out.push(0xfd);
            out.extend_from_slice(&(len as u16).to_le_bytes());
        }
        _ => {
            out.push(0xfe);
            out.extend_from_slice(&(len as u32).to_le_bytes());
        }
    }
    out
}

fn witness_commitment_script(wtxids: Vec<[u8; 32]>, reserved_value: [u8; 32]) -> ScriptBuf {
    let root = merkle_root(wtxids);
    let mut data = [0u8; 64];
    data[..32].copy_from_slice(&root);
    data[32..].copy_from_slice(&reserved_value);
    let commitment = sha256d::Hash::hash(&data).to_byte_array();
    let mut push = WITNESS_COMMITMENT_PREFIX.to_vec();
    push.extend_from_slice(&commitment);
    Builder::new()
        .push_opcode(OP_RETURN)
        .push_slice(push_bytes(push))
        .into_script()
}

fn bip34_height_bytes(height: u32) -> Vec<u8> {
    if height == 0 {
        return vec![0x00];
    }
    let mut v = Vec::new();
    let mut n = height;
    while n > 0 {
        v.push((n & 0xff) as u8);
        n >>= 8;
    }
    if v.last().is_some_and(|b| b & 0x80 != 0) {
        v.push(0x00);
    }
    v
}

// Core's BIP34 check is a bytewise prefix match against `CScript() << nHeight`,
// whose `push_int64` emits OP_0/OP_1..OP_16 for heights 0..=16 and only falls
// back to a minimal CScriptNum push from 17 up. A one-byte scriptSig also fails
// the 2..=100 byte coinbase rule, so heights 0..=16 are padded with OP_0 — the
// same shape Core's own miner produces (`CScript() << nHeight << OP_0`).
fn bip34_coinbase_scriptsig(height: u32) -> ScriptBuf {
    let bytes: Vec<u8> = match height {
        0 => vec![0x00, 0x00],
        1..=16 => vec![0x50 + height as u8, 0x00],
        _ => {
            let num = bip34_height_bytes(height);
            let mut s = Vec::with_capacity(1 + num.len());
            s.push(num.len() as u8);
            s.extend_from_slice(&num);
            s
        }
    };
    ScriptBuf::from_bytes(bytes)
}

fn build_coinbase(
    height: u32,
    value: u64,
    reward_spk: &Script,
    commitment: ScriptBuf,
) -> Transaction {
    let script_sig = bip34_coinbase_scriptsig(height);
    let mut witness = Witness::new();
    witness.push([0u8; 32]);
    Transaction {
        version: TxVersion::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig,
            sequence: Sequence::MAX,
            witness,
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(value),
                script_pubkey: reward_spk.to_owned(),
            },
            TxOut {
                value: Amount::ZERO,
                script_pubkey: commitment,
            },
        ],
    }
}

fn signet_txs(
    version: i32,
    prev_blockhash: BlockHash,
    time: u32,
    signet_merkle_root: [u8; 32],
    challenge: &Script,
) -> (Transaction, Transaction) {
    let mut block_data = Vec::with_capacity(72);
    block_data.extend_from_slice(&version.to_le_bytes());
    block_data.extend_from_slice(prev_blockhash.as_ref());
    block_data.extend_from_slice(&signet_merkle_root);
    block_data.extend_from_slice(&time.to_le_bytes());

    let to_spend = Transaction {
        version: TxVersion(0),
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: Builder::new()
                .push_opcode(bitcoin::opcodes::all::OP_PUSHBYTES_0)
                .push_slice(push_bytes(block_data))
                .into_script(),
            sequence: Sequence::ZERO,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::ZERO,
            script_pubkey: challenge.to_owned(),
        }],
    };

    let to_sign = Transaction {
        version: TxVersion(0),
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(to_spend.compute_txid(), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ZERO,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::ZERO,
            script_pubkey: Builder::new().push_opcode(OP_RETURN).into_script(),
        }],
    };

    (to_spend, to_sign)
}

fn solution_scriptsig(sig: Signature) -> ScriptBuf {
    let mut sig_bytes = sig.serialize_der().to_vec();
    sig_bytes.push(0x01);
    Builder::new()
        .push_opcode(bitcoin::opcodes::all::OP_PUSHBYTES_0)
        .push_slice(push_bytes(sig_bytes))
        .into_script()
}

pub fn assemble_block(
    tmpl: &BlockTemplate,
    reward_spk: &Script,
    key: &SecretKey,
    expected_challenge: Option<&Script>,
) -> Result<Block, SignetError> {
    let challenge = ScriptBuf::from_hex(&tmpl.signet_challenge)
        .map_err(|e| SignetError::Decode(e.to_string()))?;
    if let Some(expected) = expected_challenge
        && challenge != *expected
    {
        return Err(SignetError::ChallengeMismatch {
            template: tmpl.signet_challenge.clone(),
            configured: expected.to_hex_string(),
        });
    }

    let txs: Vec<Transaction> = tmpl
        .transactions
        .iter()
        .map(|t| deserialize_hex(&t.data).map_err(|e| SignetError::Decode(e.to_string())))
        .collect::<Result<_, _>>()?;

    let mut wtxids: Vec<[u8; 32]> = vec![[0u8; 32]];
    for tx in &txs {
        wtxids.push(tx.compute_wtxid().to_byte_array());
    }
    let commitment = witness_commitment_script(wtxids, [0u8; 32]);
    let coinbase = build_coinbase(tmpl.height, tmpl.coinbasevalue, reward_spk, commitment);

    let prev_blockhash: BlockHash = tmpl
        .previousblockhash
        .parse::<BlockHash>()
        .map_err(|e| SignetError::Decode(e.to_string()))?;
    let time = tmpl.curtime.max(tmpl.mintime);

    let mut modified_cb = coinbase.clone();
    {
        let last = modified_cb.output.last_mut().expect("coinbase has outputs");
        last.script_pubkey = append_push(last.script_pubkey.clone(), &SIGNET_HEADER);
    }
    let mut leaves = vec![modified_cb.compute_txid().to_byte_array()];
    for tx in &txs {
        leaves.push(tx.compute_txid().to_byte_array());
    }
    let signet_merkle_root = merkle_root(leaves);

    let (_to_spend, to_sign) = signet_txs(
        tmpl.version,
        prev_blockhash,
        time,
        signet_merkle_root,
        &challenge,
    );

    let sighash = SighashCache::new(&to_sign)
        .legacy_signature_hash(0, &challenge, 1)
        .map_err(|_| SignetError::Sighash)?;
    let secp = Secp256k1::new();
    let msg = Message::from_digest(sighash.to_byte_array());
    let sig = secp.sign_ecdsa(&msg, key);
    let solution_sig = solution_scriptsig(sig);

    let mut solution = compact_size(solution_sig.len());
    solution.extend_from_slice(solution_sig.as_bytes());
    solution.push(0x00);

    let mut final_cb = coinbase;
    {
        let last = final_cb.output.last_mut().expect("coinbase has outputs");
        let mut push = SIGNET_HEADER.to_vec();
        push.extend_from_slice(&solution);
        last.script_pubkey = append_push(last.script_pubkey.clone(), &push);
    }

    let mut txdata = vec![final_cb];
    txdata.extend(txs);

    let header_leaves: Vec<[u8; 32]> = txdata
        .iter()
        .map(|tx| tx.compute_txid().to_byte_array())
        .collect();
    let header = Header {
        version: BlockVersion::from_consensus(tmpl.version),
        prev_blockhash,
        merkle_root: TxMerkleNode::from_byte_array(merkle_root(header_leaves)),
        time,
        bits: CompactTarget::from_consensus(
            u32::from_str_radix(&tmpl.bits, 16).map_err(|e| SignetError::Decode(e.to_string()))?,
        ),
        nonce: 0,
    };

    let target = Target::from_compact(header.bits);
    let nonce = grind_nonce(header, target).ok_or(SignetError::NoNonce)?;
    let mut header = header;
    header.nonce = nonce;

    Ok(Block { header, txdata })
}

pub fn grind_nonce(header: Header, target: Target) -> Option<u32> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(8);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let found = std::sync::Arc::new(std::sync::Mutex::new(None));

    std::thread::scope(|scope| {
        for t in 0..threads {
            let stop = stop.clone();
            let found = found.clone();
            let mut header = header;
            scope.spawn(move || {
                let mut nonce = t as u32;
                loop {
                    if stop.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    header.nonce = nonce;
                    if target.is_met_by(header.block_hash()) {
                        *found.lock().expect("mutex poisoned") = Some(nonce);
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    nonce = match nonce.checked_add(threads as u32) {
                        Some(n) => n,
                        None => return,
                    };
                }
            });
        }
    });

    *found.lock().expect("mutex poisoned")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::secp256k1::rand;

    fn test_template(challenge_hex: &str) -> BlockTemplate {
        serde_json::from_value(serde_json::json!({
            "version": "20000000",
            "previousblockhash": "00000008819873e925422c1ff0f99f7cc9bbb232af63a077a480a3633bee1ef6",
            "transactions": [],
            "coinbasevalue": 5_000_000_000u64,
            "height": 1,
            "bits": "1e0377ae",
            "curtime": 1598918401,
            "mintime": 1598918401,
            "signet_challenge": challenge_hex,
        }))
        .unwrap()
    }

    #[test]
    fn challenge_is_one_of_one_multisig() {
        let secp = Secp256k1::new();
        let key = SecretKey::new(&mut rand::thread_rng());
        let pubkey = key.public_key(&secp);
        let challenge = challenge_for_pubkey(&pubkey);
        let hex = challenge.to_hex_string();
        assert!(hex.starts_with("5121"), "got {hex}");
        assert!(hex.ends_with("51ae"), "got {hex}");
        assert_eq!(hex.len(), 2 + 2 + 66 + 2 + 2);
    }

    #[test]
    fn merkle_root_single_leaf() {
        let leaf = [7u8; 32];
        assert_eq!(merkle_root(vec![leaf]), leaf);
    }

    #[test]
    fn bip34_coinbase_scriptsig_matches_core() {
        // Heights 0..=16 are OP_0/OP_1..OP_16 plus an OP_0 pad so the scriptSig
        // is >=2 bytes (bad-cb-length). From 17 up it is a minimal CScriptNum push.
        assert_eq!(bip34_coinbase_scriptsig(0).to_hex_string(), "0000");
        assert_eq!(bip34_coinbase_scriptsig(1).to_hex_string(), "5100");
        assert_eq!(bip34_coinbase_scriptsig(16).to_hex_string(), "6000");
        assert_eq!(bip34_coinbase_scriptsig(17).to_hex_string(), "0111");
        assert_eq!(bip34_coinbase_scriptsig(128).to_hex_string(), "028000");
        assert_eq!(
            bip34_coinbase_scriptsig(227_931).to_hex_string(),
            "035b7a03"
        );
    }

    #[test]
    fn assembled_block_verifies() {
        let secp = Secp256k1::new();
        let key = SecretKey::new(&mut rand::thread_rng());
        let pubkey = key.public_key(&secp);
        let challenge = challenge_for_pubkey(&pubkey);
        let tmpl = test_template(&challenge.to_hex_string());
        let reward_spk =
            ScriptBuf::from_hex("00140000000000000000000000000000000000000001").unwrap();

        let block = assemble_block(&tmpl, &reward_spk, &key, Some(&challenge)).unwrap();

        let target = Target::from_compact(block.header.bits);
        assert!(target.is_met_by(block.block_hash()));

        let coinbase = &block.txdata[0];
        let commitment_out = coinbase.output.last().unwrap();
        let mut instructions = commitment_out
            .script_pubkey
            .instructions()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            instructions.remove(0),
            bitcoin::script::Instruction::Op(bitcoin::opcodes::all::OP_RETURN)
        );
        let wit_push = instructions
            .remove(0)
            .push_bytes()
            .unwrap()
            .as_bytes()
            .to_vec();
        assert_eq!(&wit_push[..4], &WITNESS_COMMITMENT_PREFIX);
        let signet_push = instructions
            .remove(0)
            .push_bytes()
            .unwrap()
            .as_bytes()
            .to_vec();
        assert_eq!(&signet_push[..4], &SIGNET_HEADER);
        let solution = &signet_push[4..];

        let mut modified_cb = coinbase.clone();
        {
            let last = modified_cb.output.last_mut().unwrap();
            let mut bytes = last.script_pubkey.clone().into_bytes();
            bytes.truncate(2 + 36);
            bytes.push(4);
            bytes.extend_from_slice(&SIGNET_HEADER);
            last.script_pubkey = ScriptBuf::from_bytes(bytes);
        }
        let leaves: Vec<[u8; 32]> = vec![modified_cb.compute_txid().to_byte_array()];
        let signet_merkle_root = merkle_root(leaves);

        let prev: BlockHash = tmpl.previousblockhash.parse().unwrap();
        let mut block_data = Vec::new();
        block_data.extend_from_slice(&tmpl.version.to_le_bytes());
        block_data.extend_from_slice(&prev.to_byte_array());
        block_data.extend_from_slice(&signet_merkle_root);
        block_data.extend_from_slice(&tmpl.curtime.to_le_bytes());

        let to_spend = Transaction {
            version: TxVersion(0),
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: Builder::new()
                    .push_opcode(bitcoin::opcodes::all::OP_PUSHBYTES_0)
                    .push_slice(push_bytes(block_data))
                    .into_script(),
                sequence: Sequence::ZERO,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::ZERO,
                script_pubkey: challenge.clone(),
            }],
        };
        let mut to_sign = Transaction {
            version: TxVersion(0),
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(to_spend.compute_txid(), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ZERO,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::ZERO,
                script_pubkey: Builder::new().push_opcode(OP_RETURN).into_script(),
            }],
        };

        let ss_len = solution[0] as usize;
        let script_sig = ScriptBuf::from_bytes(solution[1..1 + ss_len].to_vec());
        assert_eq!(solution[1 + ss_len], 0x00, "empty witness stack expected");

        let sighash = SighashCache::new(&to_sign)
            .legacy_signature_hash(0, &challenge, 1)
            .unwrap();
        to_sign.input[0].script_sig = script_sig.clone();

        let sig_bytes: Vec<u8> = script_sig
            .instructions()
            .last()
            .unwrap()
            .unwrap()
            .push_bytes()
            .unwrap()
            .as_bytes()
            .to_vec();
        assert_eq!(sig_bytes.last(), Some(&0x01));
        let sig = Signature::from_der(&sig_bytes[..sig_bytes.len() - 1]).unwrap();
        let msg = Message::from_digest(sighash.to_byte_array());
        secp.verify_ecdsa(&msg, &sig, &pubkey).unwrap();

        let header_leaves: Vec<[u8; 32]> = block
            .txdata
            .iter()
            .map(|tx| tx.compute_txid().to_byte_array())
            .collect();
        assert_eq!(
            block.header.merkle_root.to_byte_array(),
            merkle_root(header_leaves)
        );
    }
}
