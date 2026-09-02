use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct BlockTemplate {
    #[serde(deserialize_with = "de_hex_i32")]
    pub version: i32,
    pub previousblockhash: String,
    #[serde(default)]
    pub transactions: Vec<TemplateTx>,
    pub coinbasevalue: u64,
    pub height: u32,
    pub bits: String,
    pub curtime: u32,
    pub mintime: u32,
    pub signet_challenge: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateTx {
    pub data: String,
    pub txid: String,
    pub hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlockchainInfo {
    pub chain: String,
    pub blocks: u32,
    pub headers: u32,
    pub bestblockhash: String,
    #[serde(rename = "signet_challenge")]
    pub signet_challenge: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddressInfo {
    pub address: String,
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: String,
}

fn de_hex_i32<'de, D>(deserializer: D) -> std::result::Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde_json::Value;
    let val = Value::deserialize(deserializer)?;
    match val {
        Value::String(s) => i64::from_str_radix(&s, 16)
            .map(|v| v as i32)
            .map_err(serde::de::Error::custom),
        Value::Number(n) => n
            .as_i64()
            .map(|v| v as i32)
            .ok_or_else(|| serde::de::Error::custom("number out of range")),
        _ => Err(serde::de::Error::custom("expected string or number")),
    }
}
