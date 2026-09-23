// Prints a NIP-98 Authorization header for one JSON-RPC call, for curl-based
// e2e checks. Dev key comes from SIGNET_DEV_KEY (64 hex); a fixed key keeps
// the npub stable across local runs.
//
//   AUTH=$(cargo run -q -p signet-api --example rpc-client -- http://localhost:8081)
//   curl -s http://localhost:8081/v1/rpc \
//     -H "Authorization: Nostr $AUTH" -H 'content-type: application/json' \
//     -d '{"jsonrpc":"2.0","id":1,"method":"environment.create","params":{...}}'
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use nostr::event::{EventBuilder, FinalizeEvent, Kind, Tag};
use nostr::key::Keys;
use nostr::types::Timestamp;

const DEFAULT_DEV_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";

// Backdate a little so slow curls never trip the freshness window.
const SKEW_BACK_SECS: u64 = 5;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let public_url = args
        .next()
        .unwrap_or_else(|| "http://localhost:8081".to_string());
    let http_method = args.next().unwrap_or_else(|| "POST".to_string());

    let hex_key = std::env::var("SIGNET_DEV_KEY").unwrap_or_else(|_| DEFAULT_DEV_KEY.to_string());
    let keys = Keys::parse(&hex_key)?;
    let created_at = Timestamp::now() - Timestamp::from_secs(SKEW_BACK_SECS);
    let event = EventBuilder::new(Kind::HttpAuth, "")
        .tags(vec![
            Tag::custom("u", [format!("{public_url}/v1/rpc")]),
            Tag::custom("method", [&http_method]),
        ])
        .custom_created_at(created_at)
        .finalize(&keys)?;

    println!("{}", BASE64.encode(serde_json::to_vec(&event)?));
    Ok(())
}
