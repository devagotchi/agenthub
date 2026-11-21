//! Commands on a held ticket's lease: claim, beat, note, done, handoff, release and force.

use crate::cli::{CachedClaim, CommandResult, EXIT_OK, no_id, usage_error, with_id};
use crate::{Filter, NextFilter, face::ME};

use super::{Command, arg, fence_for, need, optional_fence};

/// `board claim`: claim a ticket, or the first ready one, and cache its fence.
pub(super) fn claim(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        cache,
        output,
        ..
    } = c;
    let ttl = parsed.int("--ttl").map_err(usage_error)?.map(|s| s * 1000);
    let lease = match arg(parsed, 0) {
        Some(id) => with_id(id, board.claim(actor, id, ttl))?,
        None => {
            let filter = NextFilter {
                label: parsed.flag("--label").map(str::to_string),
                states: parsed.flag_all("--state"),
                parent: parsed.flag("--parent").map(str::to_string),
                ttl_ms: ttl,
            };
            match no_id(board.claim_next(actor, &filter))? {
                Some(lease) => lease,
                None => {
                    output.value(&serde_json::json!({ "claimed": false }), || {
                        "nothing ready for you".into()
                    });
                    return Ok(EXIT_OK);
                }
            }
        }
    };
    if !board.replayed() {
        cache.claims.insert(
            lease.ticket.id.clone(),
            CachedClaim {
                fence: lease.fence,
                version: lease.ticket.version,
                lease_expires_at: lease.lease_expires_at,
            },
        );
    }
    output.value(&lease, || {
        format!(
            "{}  claimed (fence {}, heartbeat every {}s): {}\n  board beat {} · board done {} \"summary\" · board handoff {} \"note\"",
            lease.ticket.id,
            lease.fence,
            lease.heartbeat_every_ms / 1000,
            lease.ticket.title,
            lease.ticket.id,
            lease.ticket.id,
            lease.ticket.id
        )
    });
    Ok(EXIT_OK)
}

/// `board beat` with no ID: heartbeat every ticket this actor holds; the hook pack runs it after each tool.
pub(super) fn beat_all(c: Command<'_, '_>) -> CommandResult {
    let Command {
        board,
        actor,
        cache,
        output,
        ..
    } = c;
    // Every ticket this actor holds, with the fence the store shows (the holder's
    // fence is by definition the current one). What the hook pack calls after tools.
    let held = no_id(board.list(
        actor,
        &Filter {
            holder: Some(ME.to_string()),
            ..Filter::default()
        },
    ))?;
    let mut beaten = Vec::new();
    for ticket in &held {
        let lease = with_id(
            &ticket.id,
            board.heartbeat(actor, &ticket.id, ticket.fence, None),
        )?;
        cache.claims.insert(
            ticket.id.clone(),
            CachedClaim {
                fence: lease.fence,
                version: lease.ticket.version,
                lease_expires_at: lease.lease_expires_at,
            },
        );
        beaten.push(lease);
    }
    output.value(&beaten, || {
        if beaten.is_empty() {
            "holding nothing".into()
        } else {
            beaten
                .iter()
                .map(|l| format!("{}  lease extended to {}", l.ticket.id, l.lease_expires_at))
                .collect::<Vec<_>>()
                .join("\n")
        }
    });
    Ok(EXIT_OK)
}

/// `board beat`: heartbeat one held ticket, with optional progress.
pub(super) fn beat(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        cache,
        output,
        ..
    } = c;
    let id = need(parsed, 0, "ID")?;
    let fence = fence_for(parsed, cache, id)?;
    let progress = match parsed.flag("--progress") {
        Some(raw) => {
            Some(serde_json::from_str(raw).map_err(|_| usage_error("--progress must be JSON"))?)
        }
        None => None,
    };
    let lease = with_id(id, board.heartbeat(actor, id, fence, progress))?;
    if !board.replayed() {
        cache.claims.insert(
            id.to_string(),
            CachedClaim {
                fence: lease.fence,
                version: lease.ticket.version,
                lease_expires_at: lease.lease_expires_at,
            },
        );
    }
    output.value(&lease, || {
        format!("{id}  lease extended to {}", lease.lease_expires_at)
    });
    Ok(EXIT_OK)
}

/// `board note`: add a note to a ticket.
pub(super) fn note(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        cache,
        output,
        ..
    } = c;
    let id = need(parsed, 0, "ID")?;
    let text = need(parsed, 1, "TEXT")?;
    let fence = optional_fence(parsed, cache, id)?;
    let note = with_id(
        id,
        board.add_note(
            actor,
            id,
            parsed.flag("--kind").unwrap_or("note"),
            text,
            parsed.flag("--to"),
            fence,
        ),
    )?;
    output.value(&note, || format!("{id}  note #{}", note.seq));
    Ok(EXIT_OK)
}

/// `board done`: complete a held ticket.
pub(super) fn done(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        cache,
        output,
        ..
    } = c;
    let id = need(parsed, 0, "ID")?;
    let fence = fence_for(parsed, cache, id)?;
    let ticket = with_id(
        id,
        board.complete(actor, id, fence, arg(parsed, 1).unwrap_or("")),
    )?;
    if !board.replayed() {
        cache.claims.remove(id);
    }
    output.value(&ticket, || format!("{id}  → {}", ticket.state));
    Ok(EXIT_OK)
}

/// `board handoff`: hand a held ticket on with a note.
pub(super) fn handoff(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        cache,
        output,
        ..
    } = c;
    let id = need(parsed, 0, "ID")?;
    let note = need(parsed, 1, "NOTE")?;
    let fence = fence_for(parsed, cache, id)?;
    let ticket = with_id(
        id,
        board.handoff(actor, id, fence, note, parsed.flag("--to")),
    )?;
    if !board.replayed() {
        cache.claims.remove(id);
    }
    output.value(&ticket, || {
        format!(
            "{id}  → {} (assigned to {})",
            ticket.state,
            ticket.assignee.as_deref().unwrap_or("anyone")
        )
    });
    Ok(EXIT_OK)
}

/// `board release`: release a held ticket.
pub(super) fn release(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        cache,
        output,
        ..
    } = c;
    let id = need(parsed, 0, "ID")?;
    let fence = fence_for(parsed, cache, id)?;
    let ticket = with_id(
        id,
        board.release(actor, id, fence, arg(parsed, 1).unwrap_or("")),
    )?;
    if !board.replayed() {
        cache.claims.remove(id);
    }
    output.value(&ticket, || format!("{id}  → {}", ticket.state));
    Ok(EXIT_OK)
}

/// `board force`: take a lease away (a person only).
pub(super) fn force(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        output,
        ..
    } = c;
    let id = need(parsed, 0, "ID")?;
    let reason = need(parsed, 1, "REASON")?;
    let ticket = with_id(id, board.force_release(actor, id, reason))?;
    output.value(&ticket, || {
        format!("{id}  → {} (lease taken away)", ticket.state)
    });
    Ok(EXIT_OK)
}
