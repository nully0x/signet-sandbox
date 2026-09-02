use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use nostr::event::{EventBuilder, FinalizeEvent, Kind, Tag};
use nostr::key::Keys;
use nostr::types::Timestamp;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .expect("usage: nip98 <url> <method> <secret-hex>");
    let method = args
        .next()
        .expect("usage: nip98 <url> <method> <secret-hex>");
    let secret = args
        .next()
        .expect("usage: nip98 <url> <method> <secret-hex>");

    let keys = Keys::parse(&secret)?;
    let event = EventBuilder::new(Kind::HttpAuth, "")
        .tags(vec![
            Tag::custom("u", [url.as_str()]),
            Tag::custom("method", [method.as_str()]),
        ])
        .custom_created_at(Timestamp::now())
        .finalize(&keys)?;
    println!(
        "Authorization: Nostr {}",
        BASE64.encode(serde_json::to_vec(&event)?)
    );
    Ok(())
}
