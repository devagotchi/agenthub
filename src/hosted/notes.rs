//! Notes on a ticket: what an agent learned, a handoff's note, a decision's note.

use super::*;
use crate::chain::actions;

impl HostedBoard {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn insert_note(
        &self,
        connection: &mut AnyConnection,
        actor: &str,
        now: i64,
        id: &str,
        kind: &str,
        body: &str,
        to: Option<&str>,
        fence: Option<i64>,
    ) -> DbResult<Note> {
        let seq: i64 = sqlx::query_scalar(
            "SELECT coalesce(max(seq),0)+1 FROM board_notes WHERE tenant_id=$1 AND board_id=$2",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .fetch_one(&mut *connection)
        .await?;
        sqlx::query(
            "INSERT INTO board_notes (tenant_id,board_id,seq,ticket,author,kind,body,to_principal,fence,created_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        ).bind(&self.tenant).bind(&self.project).bind(seq).bind(id).bind(actor).bind(kind)
            .bind(body).bind(to).bind(fence).bind(now).execute(&mut *connection).await?;
        Ok(Note {
            seq,
            ticket: id.into(),
            author: actor.into(),
            kind: kind.into(),
            body: body.into(),
            to_principal: to.map(str::to_string),
            fence,
            created_at: now,
        })
    }

    pub async fn add_note(
        &self,
        actor: &Actor<'_>,
        id: &str,
        kind: &str,
        body: &str,
        to: Option<&str>,
        fence: Option<i64>,
    ) -> BridgeResult<Note> {
        let body = body.trim();
        if body.is_empty() || body.chars().count() > 20_000 {
            return Err(invalid("note body must be 1 to 20000 characters"));
        }
        let kind = if kind.trim().is_empty() {
            "note"
        } else {
            kind.trim()
        };
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            if ticket.holder.as_deref() == Some(actor.id) && !ticket.lease_expired {
                crate::lease::check_held(&ticket, actor.id, fence)?;
            }
            let note = self
                .insert_note(connection, actor.id, now, id, kind, body, to, fence)
                .await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::NOTE_ADDED,
                Some(id),
                fence,
                serde_json::json!({"seq":note.seq,"kind":kind,"to":to}),
            )
            .await?;
            Ok(note)
        })
    }
}
