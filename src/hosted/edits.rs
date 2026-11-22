//! Guards shared by ticket edits. Lease state comes from the transaction's database clock.

use crate::{Actor, Ticket, fail};
use devagotchi_contracts::BridgeResult;

pub(super) fn unheld_edit(current: &Ticket, version: i64) -> BridgeResult<()> {
    if current.kind == crate::approval::KIND {
        return Err(fail(
            "approval_decide_only",
            "an approval changes only through a person's decision",
        ));
    }
    if current.version != version {
        return Err(fail(
            "version_conflict",
            format!(
                "version {version} is not current ({}); re-read and retry",
                current.version
            ),
        ));
    }
    Ok(())
}

/// October's update fence guard: a supplied stale fence never becomes an unfenced edit.
/// A person can edit metadata through existing board authority; another agent cannot
/// overwrite a live holder. State changes still go through the workflow/lease guard.
pub(super) fn update_fence(
    current: &Ticket,
    actor: &Actor<'_>,
    fence: Option<i64>,
) -> BridgeResult<()> {
    supplied_fence(current, fence)?;
    if current.holder.is_some() && !current.lease_expired {
        if current.holder.as_deref() == Some(actor.id) {
            crate::lease::check_held(current, actor.id, fence)?;
        } else if !actor.human {
            return Err(fail("not_holder", "another worker holds this ticket"));
        }
    }
    Ok(())
}

pub(super) fn supplied_fence(current: &Ticket, fence: Option<i64>) -> BridgeResult<()> {
    if fence.is_some_and(|fence| fence != current.fence) {
        return Err(fail(
            "stale_fence",
            "your fence is no longer current; re-read the ticket",
        ));
    }
    Ok(())
}
