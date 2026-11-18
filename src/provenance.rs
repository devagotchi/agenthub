//! Where an imported ticket came from: the source document and item a caller names when it
//! creates the ticket, kept once per board and never changed. `(path, item)` is the ticket's
//! import identity on its board: creating it again with the same inputs returns the ticket as
//! it is now; with changed inputs it is `source_conflict`. The evidence is what the caller
//! reported: the board never opens the path or checks the digest.

use devagotchi_contracts::BridgeResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{NewTicket, checks::invalid};

/// The document and item a ticket is imported from, as the importer saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketSource {
    /// Repository-relative path of the document, with forward slashes.
    pub path: String,
    /// The item's stable id within the document: not a line number or a title.
    pub item: String,
    /// The revision of the document the importer read (a commit, a tag). Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Lowercase hex SHA-256 of the item's content as the importer read it. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
}

impl TicketSource {
    pub(crate) fn validate(&self) -> BridgeResult<()> {
        for (name, value, max) in [
            ("path", Some(self.path.as_str()), 1024),
            ("item", Some(self.item.as_str()), 256),
            ("revision", self.revision.as_deref(), 256),
        ] {
            let Some(value) = value else { continue };
            if value.is_empty()
                || value != value.trim()
                || value.chars().count() > max
                || value.chars().any(char::is_control)
            {
                return Err(invalid(format!(
                    "source {name} must be 1 to {max} characters without surrounding whitespace or controls"
                )));
            }
        }
        if self.path.contains(['\\', ':'])
            || self
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".." || part != part.trim())
        {
            return Err(invalid(
                "source path must be repository-relative, with forward slashes and no empty, . or .. parts",
            ));
        }
        if let Some(digest) = &self.content_sha256
            && (digest.len() != 64
                || !digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(invalid(
                "source content_sha256 must be 64 lowercase hexadecimal characters",
            ));
        }
        Ok(())
    }
}

/// A ticket's source as the board recorded it when the ticket was made. Never changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketProvenance {
    pub source: TicketSource,
    /// The principal that created the ticket: the board's actor, never the caller's say-so.
    pub recorded_by: String,
    /// The board's clock in the creating transaction (ms since the epoch).
    pub recorded_at: i64,
    /// SHA-256 of the normalized creation inputs: what a retry must match.
    pub request_sha256: String,
}

/// The normalized creation inputs of `new`, as a digest. Normalized: the title trimmed, the
/// kind's default spelled out, the labels cleaned; a priority left out stays left out, so a
/// change of the board's default never turns a retry into a conflict. The anchors the hub
/// fills from the connection or the board's settings when a caller leaves them out (machine,
/// repo, agent, session) are where a ticket was made from, not what was asked for, and the
/// sightings (`occurrences`, `last_seen_at`) change between scans: none of them count.
pub(crate) fn request_digest(
    new: &NewTicket,
    title: &str,
    kind: &str,
    labels: &[String],
    environment: Option<&str>,
    fingerprint: Option<&str>,
) -> String {
    let inputs = serde_json::json!({
        "version": 1,
        "title": title,
        "description": new.description,
        "kind": kind,
        "priority": new.priority,
        "labels": labels,
        "parent": new.parent,
        "assignee": new.assignee,
        "discovered_from": new.discovered_from,
        "environment": environment,
        "fingerprint": fingerprint,
        "source": new.source,
    });
    hex::encode(Sha256::digest(inputs.to_string().as_bytes()))
}
