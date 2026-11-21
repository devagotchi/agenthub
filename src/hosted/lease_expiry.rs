//! Lease removal and expiry, shared by operations and the reaper.
use super::*;
use crate::chain::actions;

impl HostedBoard {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn clear_lease(
        &self,
        connection: &mut AnyConnection,
        actor: &str,
        now: i64,
        ticket: &Ticket,
        to: &str,
        action: &str,
        mut data: serde_json::Value,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE board_tickets SET holder=NULL,lease_id=NULL,lease_ttl_ms=NULL,lease_expires_at=NULL,
             state=$4,version=version+1,updated_at=$5 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
        ).bind(&self.tenant).bind(&self.project).bind(&ticket.id).bind(to).bind(now)
            .execute(&mut *connection).await?;
        data["from"] = serde_json::json!(ticket.state);
        data["to"] = serde_json::json!(to);
        self.event(
            connection,
            actor,
            now,
            action,
            Some(&ticket.id),
            Some(ticket.fence),
            data,
        )
        .await
    }

    pub(super) async fn expire(
        &self,
        connection: &mut AnyConnection,
        actor: &str,
        now: i64,
        ticket: &Ticket,
        workflow: &Workflow,
        settings: &Settings,
    ) -> DbResult<()> {
        let count = ticket.expiry_count + 1;
        let mut to = crate::lease::release_state(workflow, ticket);
        if settings.max_lease_expiries > 0 && count >= settings.max_lease_expiries {
            to = workflow.attention_state.clone();
        }
        sqlx::query(
            "UPDATE board_tickets SET expiry_count=$4 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(&ticket.id)
        .bind(count)
        .execute(&mut *connection)
        .await?;
        self.clear_lease(
            connection,
            actor,
            now,
            ticket,
            &to,
            actions::LEASE_EXPIRED,
            serde_json::json!({"previous_holder":ticket.holder,"expiry_count":count}),
        )
        .await
    }
}
