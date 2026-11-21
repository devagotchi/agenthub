//! Human delegation: recipient, note and assignment commit together.
use super::edits::unheld_edit;
use super::*;
use crate::chain::actions;

impl HostedBoard {
    /// A person hands `id` to `to` with a note saying so, as they saw it at `version`: the
    /// assignee and the note in one transaction, one `ticket.updated` naming both.
    pub async fn delegate(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        to: &str,
        note: &str,
    ) -> BridgeResult<Ticket> {
        if !actor.human {
            return Err(fail(
                "humans_only",
                "delegation needs a person; a worker uses handoff with its lease fence",
            ));
        }
        let to = to.trim();
        let note = note.trim();
        if note.is_empty() || note.chars().count() > 20_000 {
            return Err(invalid("note body must be 1 to 20000 characters"));
        }
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let current = self.ticket(connection, id, &workflow, now).await?;
            unheld_edit(&current, version)?;
            self.authorize(connection, actor, "task.edit", &current.labels)
                .await?;
            self.recipient(connection, actor, to).await?;
            sqlx::query(
                "UPDATE board_tickets SET assignee=$4,version=version+1,updated_at=$5
                 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(id)
            .bind(to)
            .bind(now)
            .execute(&mut *connection)
            .await?;
            let note = self
                .insert_note(connection, actor.id, now, id, "note", note, Some(to), None)
                .await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::TICKET_UPDATED,
                Some(id),
                None,
                serde_json::json!({"assignee": to, "note_seq": note.seq}),
            )
            .await?;
            self.ticket(connection, id, &workflow, now).await
        })
    }
}
