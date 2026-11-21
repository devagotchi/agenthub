//! Leases: claim (one ticket or the next ready one), heartbeat, complete, release, handoff,
//! force-release and the reaper, each one transaction on the database clock.

use super::*;
use crate::chain::actions;
use crate::lease::projection as lease;

impl HostedBoard {
    pub async fn heartbeat(
        &self,
        actor: &Actor<'_>,
        id: &str,
        fence: i64,
        progress: Option<serde_json::Value>,
    ) -> BridgeResult<Lease> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            let settings: Settings = self.configuration(connection, "settings").await?;
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            let receipt = self.begin_mutation(connection, actor, "heartbeat").await?;
            if let Receipt::Replay(saved) = receipt {
                return Ok(saved);
            }
            crate::lease::check_held(&ticket, actor.id, Some(fence))?;
            let ttl = ticket.lease_ttl_ms.unwrap_or(settings.lease_ttl_ms);
            sqlx::query(
                "UPDATE board_tickets SET lease_expires_at=$4,heartbeat_at=$5,
                 progress=coalesce($6,progress),updated_at=$5 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
            ).bind(&self.tenant).bind(&self.project).bind(id).bind(now+ttl).bind(now)
                .bind(progress.as_ref().map(serde_json::Value::to_string))
                .execute(&mut *connection).await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::LEASE_EXTENDED,
                Some(id),
                Some(fence),
                serde_json::json!({"lease_expires_at":now+ttl,"progress":progress}),
            )
            .await?;
            let result = lease(
                self.ticket(connection, id, &workflow, now).await?,
                &settings,
                true,
            )?;
            receipt.complete(connection, result).await
        })
    }

    pub async fn complete(
        &self,
        actor: &Actor<'_>,
        id: &str,
        fence: i64,
        summary: &str,
    ) -> BridgeResult<Ticket> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            let receipt: Receipt<(Ticket, String)> =
                self.begin_mutation(connection, actor, "complete").await?;
            if let Receipt::Replay((saved, action)) = receipt {
                self.authorize(connection, actor, &action, &ticket.labels)
                    .await?;
                return Ok(saved);
            }
            crate::lease::check_held(&ticket, actor.id, Some(fence))?;
            let to = workflow
                .lane_by_active(&ticket.state)
                .map(|lane| lane.complete_to.clone())
                .ok_or_else(|| fail("not_completable", "this state has no active work stage; repair the workflow before completing"))?;
            self.authorize(
                connection,
                actor,
                &format!("task.complete:{}", ticket.state),
                &ticket.labels,
            )
            .await?;
            if !summary.trim().is_empty() {
                self.insert_note(
                    connection,
                    actor.id,
                    now,
                    id,
                    "summary",
                    summary.trim(),
                    None,
                    Some(fence),
                )
                .await?;
            }
            sqlx::query(
                "UPDATE board_tickets SET last_worker=$4,closed_at=coalesce($5,closed_at)
                 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(id)
            .bind(actor.id)
            .bind(workflow.is_done(&to).then_some(now))
            .execute(&mut *connection)
            .await?;
            self.clear_lease(
                connection,
                actor.id,
                now,
                &ticket,
                &to,
                actions::TICKET_COMPLETED,
                serde_json::json!({"summary":!summary.trim().is_empty()}),
            )
            .await?;
            let result = self.ticket(connection, id, &workflow, now).await?;
            receipt
                .complete(
                    connection,
                    (result, format!("task.complete:{}", ticket.state)),
                )
                .await
                .map(|(ticket, _)| ticket)
        })
    }

    pub async fn release(
        &self,
        actor: &Actor<'_>,
        id: &str,
        fence: i64,
        reason: &str,
    ) -> BridgeResult<Ticket> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            let receipt = self.begin_mutation(connection, actor, "release").await?;
            if let Receipt::Replay(saved) = receipt {
                return Ok(saved);
            }
            crate::lease::check_held(&ticket, actor.id, Some(fence))?;
            self.clear_lease(
                connection,
                actor.id,
                now,
                &ticket,
                &crate::lease::release_state(&workflow, &ticket),
                actions::TICKET_RELEASED,
                serde_json::json!({"reason":reason}),
            )
            .await?;
            let result = self.ticket(connection, id, &workflow, now).await?;
            receipt.complete(connection, result).await
        })
    }

    pub async fn handoff(
        &self,
        actor: &Actor<'_>,
        id: &str,
        fence: i64,
        note: &str,
        to: Option<&str>,
    ) -> BridgeResult<Ticket> {
        let note = note.trim();
        if note.is_empty() {
            return Err(invalid("a handoff needs a note for whoever picks it up"));
        }
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            self.authorize(connection, actor, "task.handoff", &ticket.labels)
                .await?;
            if let Some(to) = to {
                self.recipient(connection, actor, to).await?;
            }
            let receipt = self.begin_mutation(connection, actor, "handoff").await?;
            if let Receipt::Replay(saved) = receipt {
                return Ok(saved);
            }
            crate::lease::check_held(&ticket, actor.id, Some(fence))?;
            self.insert_note(
                connection,
                actor.id,
                now,
                id,
                "handoff",
                note,
                to,
                Some(fence),
            )
            .await?;
            sqlx::query(
                "UPDATE board_tickets SET assignee=$4,last_worker=$5 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(id)
            .bind(to)
            .bind(actor.id)
            .execute(&mut *connection)
            .await?;
            self.clear_lease(
                connection,
                actor.id,
                now,
                &ticket,
                &crate::lease::release_state(&workflow, &ticket),
                actions::TICKET_HANDED_OFF,
                serde_json::json!({"assignee":to}),
            )
            .await?;
            let result = self.ticket(connection, id, &workflow, now).await?;
            receipt.complete(connection, result).await
        })
    }

    pub async fn force_release(
        &self,
        actor: &Actor<'_>,
        id: &str,
        reason: &str,
    ) -> BridgeResult<Ticket> {
        if !actor.human {
            return Err(fail("humans_only", "force-release needs a human principal"));
        }
        if reason.trim().chars().count() < 3 {
            return Err(invalid("a reason of at least 3 characters is required"));
        }
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            self.authorize(connection, actor, "task.force_release", &ticket.labels)
                .await?;
            let receipt = self
                .begin_mutation(connection, actor, "force_release")
                .await?;
            if let Receipt::Replay(saved) = receipt {
                return Ok(saved);
            }
            if ticket.holder.is_none() {
                return Err(fail("not_held", format!("{id} is not held")).into());
            }

            self.insert_note(
                connection,
                actor.id,
                now,
                id,
                "note",
                &format!(
                    "Force-released from {}: {}",
                    ticket.holder.as_deref().unwrap_or_default(),
                    reason.trim()
                ),
                None,
                None,
            )
            .await?;
            self.clear_lease(connection,actor.id,now,&ticket,&crate::lease::release_state(&workflow,&ticket),
                actions::TICKET_FORCE_RELEASED,serde_json::json!({"previous_holder":ticket.holder,"previous_fence":ticket.fence,"reason":reason.trim()})).await?;
            let result = self.ticket(connection, id, &workflow, now).await?;
            receipt.complete(connection, result).await
        })
    }

    pub async fn reap(&self) -> BridgeResult<usize> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            let settings = self.configuration(connection, "settings").await?;
            let ids:Vec<String>=sqlx::query_scalar(
                "SELECT id FROM board_tickets WHERE tenant_id=$1 AND board_id=$2 AND holder IS NOT NULL AND lease_expires_at<=$3 ORDER BY id COLLATE \"C\"",
            ).bind(&self.tenant).bind(&self.project).bind(now).fetch_all(&mut *connection).await?;
            for id in &ids {
                let ticket = self.ticket(connection, id, &workflow, now).await?;
                self.expire(connection, "system", now, &ticket, &workflow, &settings)
                    .await?;
            }
            Ok(ids.len())
        })
    }

    pub async fn next_lease_expiry(&self) -> BridgeResult<Option<i64>> {
        transaction!(&self.store, |connection| {
            self.scope(connection).await?;
            Ok(sqlx::query_scalar(
                "SELECT min(lease_expires_at) FROM board_tickets WHERE tenant_id=$1 AND board_id=$2 AND holder IS NOT NULL",
            ).bind(&self.tenant).bind(&self.project).fetch_one(&mut *connection).await?)
        })
    }
}
