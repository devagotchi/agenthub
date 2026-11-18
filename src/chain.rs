//! The hash chain every board write extends: the genesis link and the hash.

use sha2::{Digest, Sha256};

use crate::Event;

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub fn event_hash(prev: &str, event: &Event) -> String {
    // Canonical: fixed field order, no whitespace, the same bytes any verifier recomputes.
    let canonical = serde_json::json!({
        "seq": event.seq,
        "at": event.at,
        "actor": event.actor,
        "action": event.action,
        "ticket": event.ticket,
        "data": event.data,
        "fence": event.fence,
    });
    let mut hasher = Sha256::new();
    hasher.update(prev.as_bytes());
    hasher.update(
        serde_json::to_string(&canonical)
            .expect("canonical json")
            .as_bytes(),
    );
    hex::encode(hasher.finalize())
}
