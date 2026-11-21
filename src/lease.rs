//! Claims and leases: the protocol that lets several subscriptions work one board
//! without stepping on each other. Ported from the Go hub's `store/claim.go`,
//! `lease.go` and `reaper.go`; the fence is the whole story — every mutation of a
//! held ticket carries it, a lapsed lease hands out a new one, and the old number
//! is never reused, so a worker that wakes up late is rejected, not merged.

use devagotchi_contracts::BridgeResult;
use serde::{Deserialize, Serialize};

use super::{Actor, Board, Settings, Ticket, fail, workflow::Workflow};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Lease {
    pub ticket: Ticket,
    pub fence: i64,
    pub lease_id: String,
    pub lease_expires_at: i64,
    /// About a third of the ttl: heartbeat at least this often.
    pub heartbeat_every_ms: i64,
    /// The caller already held it; the current lease is returned, nothing changed.
    pub already_held: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NextFilter {
    pub label: Option<String>,
    /// All of these labels must be present, together with the singular label if given.
    pub labels: Vec<String>,
    pub states: Vec<String>,
    pub parent: Option<String>,
    pub ttl_ms: Option<i64>,
}

/// Holder and fence must match and the lease must be live. Shared by every
/// mutation of a held ticket.
pub fn check_held(ticket: &Ticket, actor: &str, fence: Option<i64>) -> BridgeResult<()> {
    let Some(fence) = fence else {
        return Err(fail("stale_fence", "fence is required for a held ticket"));
    };
    if fence != ticket.fence {
        return Err(fail(
            "stale_fence",
            format!(
                "fence {fence} is not current ({}); your lease was lost",
                ticket.fence
            ),
        ));
    }
    if ticket.holder.as_deref() != Some(actor) {
        return Err(fail("not_holder", format!("you do not hold {}", ticket.id)));
    }
    if ticket.lease_expired {
        return Err(fail(
            "lease_expired",
            format!(
                "lease expired at {}; claim again",
                ticket.lease_expires_at.unwrap_or_default()
            ),
        ));
    }
    Ok(())
}

impl Board {
    /// Take a lease on one ticket. Re-claiming a ticket you already hold returns the
    /// current lease; a lapsed lease held by someone else is expired first.
    pub fn claim(&self, actor: &Actor<'_>, id: &str, ttl_ms: Option<i64>) -> BridgeResult<Lease> {
        self.hosted_call(self.hosted.claim(actor, id, ttl_ms))
    }

    /// Claim the best ticket the actor may take: assigned to them first, then
    /// priority, then age. `None` when nothing is ready.
    pub fn claim_next(
        &self,
        actor: &Actor<'_>,
        filter: &NextFilter,
    ) -> BridgeResult<Option<Lease>> {
        self.hosted_call(self.hosted.claim_next(actor, filter))
    }

    /// Extend a live lease by its ttl; optionally record progress for watchers.
    pub fn heartbeat(
        &self,
        actor: &Actor<'_>,
        id: &str,
        fence: i64,
        progress: Option<serde_json::Value>,
    ) -> BridgeResult<Lease> {
        self.hosted_call(self.hosted.heartbeat(actor, id, fence, progress))
    }

    /// Give the ticket back to its lane's release state.
    pub fn release(
        &self,
        actor: &Actor<'_>,
        id: &str,
        fence: i64,
        reason: &str,
    ) -> BridgeResult<Ticket> {
        self.hosted_call(self.hosted.release(actor, id, fence, reason))
    }

    /// Finish the lane: the ticket moves to the lane's `complete_to` and records
    /// the worker, so the next lane can require someone else.
    pub fn complete(
        &self,
        actor: &Actor<'_>,
        id: &str,
        fence: i64,
        summary: &str,
    ) -> BridgeResult<Ticket> {
        self.hosted_call(self.hosted.complete(actor, id, fence, summary))
    }

    /// A handoff note, a release, and an assignee, in one transaction.
    pub fn handoff(
        &self,
        actor: &Actor<'_>,
        id: &str,
        fence: i64,
        note: &str,
        to: Option<&str>,
    ) -> BridgeResult<Ticket> {
        self.hosted_call(self.hosted.handoff(actor, id, fence, note, to))
    }

    /// A person takes a lease away. Code rule, not policy: only a human may, and only with a reason.
    pub fn force_release(&self, actor: &Actor<'_>, id: &str, reason: &str) -> BridgeResult<Ticket> {
        self.hosted_call(self.hosted.force_release(actor, id, reason))
    }

    /// Expire every lapsed lease, by the database's clock. Returns how many. Run it on an
    /// interval and lazily inside claims, as the Go hub did.
    pub fn reap(&self) -> BridgeResult<usize> {
        self.hosted_call(self.hosted.reap())
    }
}

pub fn beat_every(ttl_ms: i64) -> i64 {
    (ttl_ms / 3).max(1_000)
}

pub(crate) fn projection(
    ticket: Ticket,
    settings: &Settings,
    already_held: bool,
) -> BridgeResult<Lease> {
    let ttl = ticket.lease_ttl_ms.unwrap_or(settings.lease_ttl_ms);
    Ok(Lease {
        fence: ticket.fence,
        lease_id: ticket
            .lease_id
            .clone()
            .ok_or_else(|| fail("board_damaged", "held ticket has no lease ID"))?,
        lease_expires_at: ticket
            .lease_expires_at
            .ok_or_else(|| fail("board_damaged", "held ticket has no lease expiry"))?,
        heartbeat_every_ms: settings.heartbeat_every_ms(ttl),
        already_held,
        ticket,
    })
}

pub fn clamp_ttl(settings: &Settings, asked: Option<i64>) -> i64 {
    asked
        .filter(|ttl| *ttl > 0)
        .unwrap_or(settings.lease_ttl_ms)
        .clamp(settings.lease_min_ttl_ms, settings.lease_max_ttl_ms)
}

pub fn release_state(workflow: &Workflow, ticket: &Ticket) -> String {
    workflow
        .lane_by_active(&ticket.state)
        .map(|l| l.release_to.clone())
        .unwrap_or_else(|| workflow.initial_state.clone())
}
