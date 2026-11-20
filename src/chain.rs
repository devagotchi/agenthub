//! The hash chain every board write extends: the genesis link and the hash.

use sha2::{Digest, Sha256};

use crate::Event;

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Every action an event on the chain names, `<subject>.<past participle>`: also the outbox
/// `kind` signal sinks and ClickHouse receive. An event keeps the name it was appended with
/// (the hash covers it), so a rename shows only on new events; the five renamed in B3 are
/// noted where they changed.
pub mod actions {
    pub const TICKET_CREATED: &str = "ticket.created";
    pub const TICKET_UPDATED: &str = "ticket.updated";
    pub const TICKET_CLAIMED: &str = "ticket.claimed";
    pub const TICKET_COMPLETED: &str = "ticket.completed";
    pub const TICKET_RELEASED: &str = "ticket.released";
    /// Was `ticket.handoff` before B3.
    pub const TICKET_HANDED_OFF: &str = "ticket.handed_off";
    pub const TICKET_FORCE_RELEASED: &str = "ticket.force_released";
    /// Was `ticket.heartbeat` before B3.
    pub const LEASE_EXTENDED: &str = "lease.extended";
    pub const LEASE_EXPIRED: &str = "lease.expired";
    pub const NOTE_ADDED: &str = "note.added";
    pub const DEP_ADDED: &str = "dep.added";
    pub const DEP_REMOVED: &str = "dep.removed";
    pub const APPROVAL_REQUESTED: &str = "approval.requested";
    pub const APPROVAL_DECIDED: &str = "approval.decided";
    pub const APPROVAL_STANDING_GRANTED: &str = "approval.standing_granted";
    pub const APPROVAL_STANDING_REVOKED: &str = "approval.standing_revoked";
    pub const BOARD_WELCOMED: &str = "board.welcomed";
    pub const BOARD_SETTINGS_CHANGED: &str = "board.settings_changed";
    /// Was `admin.settings.put` before B3.
    pub const BOARD_SETTINGS_REPLACED: &str = "board.settings_replaced";
    /// Was `admin.workflow.put` before B3.
    pub const BOARD_WORKFLOW_REPLACED: &str = "board.workflow_replaced";
    pub const ISSUE_PROJECTED: &str = "issue.projected";
    pub const ISSUE_CONTEXT_ADDED: &str = "issue.context_added";
    /// Was `notes.wake_set` before B3.
    pub const WAKE_SET: &str = "wake.set";

    /// Every action, in the order above.
    pub const ALL: [&str; 23] = [
        TICKET_CREATED,
        TICKET_UPDATED,
        TICKET_CLAIMED,
        TICKET_COMPLETED,
        TICKET_RELEASED,
        TICKET_HANDED_OFF,
        TICKET_FORCE_RELEASED,
        LEASE_EXTENDED,
        LEASE_EXPIRED,
        NOTE_ADDED,
        DEP_ADDED,
        DEP_REMOVED,
        APPROVAL_REQUESTED,
        APPROVAL_DECIDED,
        APPROVAL_STANDING_GRANTED,
        APPROVAL_STANDING_REVOKED,
        BOARD_WELCOMED,
        BOARD_SETTINGS_CHANGED,
        BOARD_SETTINGS_REPLACED,
        BOARD_WORKFLOW_REPLACED,
        ISSUE_PROJECTED,
        ISSUE_CONTEXT_ADDED,
        WAKE_SET,
    ];
}

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
