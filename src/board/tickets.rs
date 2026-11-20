//! Changing tickets: creating one, updating it along the workflow, and adding a note.

use devagotchi_contracts::BridgeResult;

use crate::{Actor, Board, NewTicket, Note, Patch, Ticket};

impl Board {
    pub fn create(&self, actor: &Actor<'_>, new: NewTicket) -> BridgeResult<Ticket> {
        self.hosted_call(self.hosted.create(actor, &new))
    }

    /// Edit fields and, for an unheld ticket, move it along a workflow transition.
    /// `version` must match (optimistic concurrency); held tickets refuse state changes.
    pub fn update(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        patch: Patch,
    ) -> BridgeResult<Ticket> {
        self.hosted_call(self.hosted.update(actor, id, version, &patch))
    }

    /// Append a note. A note on a ticket the author holds carries the fence, which
    /// is checked; anyone may annotate an unheld ticket without one.
    pub fn add_note(
        &self,
        actor: &Actor<'_>,
        id: &str,
        kind: &str,
        body: &str,
        to_principal: Option<&str>,
        fence: Option<i64>,
    ) -> BridgeResult<Note> {
        self.hosted_call(
            self.hosted
                .add_note(actor, id, kind, body, to_principal, fence),
        )
    }

    /// A person hands `id` to `to` with a note saying so, in one step.
    pub fn delegate(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        to: &str,
        note: &str,
    ) -> BridgeResult<Ticket> {
        self.hosted_call(self.hosted.delegate(actor, id, version, to, note))
    }

    /// Reopen a ticket closed moments ago (`UNDO_MS`), back to `from` when the workflow
    /// allows it.
    pub fn undo_close(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        from: &str,
    ) -> BridgeResult<Ticket> {
        self.hosted_call(self.hosted.undo_close(actor, id, version, from))
    }
}
