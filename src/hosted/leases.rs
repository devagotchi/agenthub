//! Leases: claim (one ticket or the next ready one), heartbeat, complete, release, handoff,
//! force-release and the reaper, each one transaction on the database clock.

use super::*;
use crate::Filter;
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

    pub async fn claim(
        &self,
        actor: &Actor<'_>,
        id: &str,
        ttl_ms: Option<i64>,
    ) -> BridgeResult<Lease> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let settings: Settings = self.configuration(connection, "settings").await?;
            self.claim_in(connection, actor, id, ttl_ms, now, &workflow, &settings)
                .await
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn claim_in(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        id: &str,
        ttl_ms: Option<i64>,
        now: i64,
        workflow: &Workflow,
        settings: &Settings,
    ) -> DbResult<Lease> {
        let mut ticket = self.ticket(connection, id, workflow, now).await?;
        if ticket.holder.is_some() && !ticket.lease_expired {
            if ticket.holder.as_deref() == Some(actor.id) {
                return Ok(lease(ticket, settings, true)?);
            }
            return Err(fail(
                "held_by_other",
                format!("{id} is leased by another principal"),
            )
            .into());
        }
        if ticket.holder.is_some() {
            self.expire(connection, actor.id, now, &ticket, workflow, settings)
                .await?;
            ticket = self.ticket(connection, id, workflow, now).await?;
        }
        let lane = workflow.lane_by_queue(&ticket.state).ok_or_else(|| {
            fail(
                "not_claimable",
                format!("state {:?} is not a claimable queue", ticket.state),
            )
        })?;
        if !ticket.blocked_by.is_empty() {
            return Err(fail(
                "blocked",
                format!("waiting on {}", ticket.blocked_by.join(", ")),
            )
            .into());
        }
        if lane.distinct_from_previous && ticket.last_worker.as_deref() == Some(actor.id) {
            return Err(fail(
                "distinct_required",
                "this lane requires a different principal",
            )
            .into());
        }
        self.check_wip(connection, actor, settings, now).await?;
        let ttl = crate::lease::clamp_ttl(settings, ttl_ms);
        let fence: i64 = sqlx::query_scalar(
                "UPDATE board_fence SET value=value+1 WHERE tenant_id=$1 AND board_id=$2 AND id=1 RETURNING value",
            ).bind(&self.tenant).bind(&self.project).fetch_one(&mut *connection).await?;
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|error| fail("board_random", error.to_string()))?;
        sqlx::query(
            "UPDATE board_tickets SET holder=$4,lease_id=$5,lease_ttl_ms=$6,lease_expires_at=$7,
                 heartbeat_at=$8,fence=$9,state=$10,version=version+1,updated_at=$8
                 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(id)
        .bind(actor.id)
        .bind(hex::encode(random))
        .bind(ttl)
        .bind(now + ttl)
        .bind(now)
        .bind(fence)
        .bind(&lane.active)
        .execute(&mut *connection)
        .await?;
        self.event(connection, actor.id, now, actions::TICKET_CLAIMED, Some(id), Some(fence),
                serde_json::json!({"from":ticket.state,"to":lane.active,"ttl_ms":ttl,"lease_expires_at":now+ttl})).await?;
        Ok(lease(
            self.ticket(connection, id, workflow, now).await?,
            settings,
            false,
        )?)
    }

    /// `wip_limit` when `actor` already holds as many live leases as its limit (its own, else
    /// the board's default; 0 means none). Claim and claim-next both ask this.
    async fn check_wip(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        settings: &Settings,
        now: i64,
    ) -> DbResult<()> {
        let limit = actor.wip_limit.unwrap_or(settings.wip_limit_default);
        if limit <= 0 {
            return Ok(());
        }
        let held: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM board_tickets
             WHERE tenant_id=$1 AND board_id=$2 AND holder=$3 AND lease_expires_at>$4",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(actor.id)
        .bind(now)
        .fetch_one(&mut *connection)
        .await?;
        if held >= limit {
            return Err(fail(
                "wip_limit",
                format!("you already hold {held} ticket(s); the WIP limit is {limit}"),
            )
            .into());
        }
        Ok(())
    }

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
            Ok(lease(
                self.ticket(connection, id, &workflow, now).await?,
                &settings,
                true,
            )?)
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
            crate::lease::check_held(&ticket, actor.id, Some(fence))?;
            let to = workflow
                .lane_by_active(&ticket.state)
                .map(|lane| lane.complete_to.clone())
                .unwrap_or_else(|| workflow.initial_state.clone());
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
            self.ticket(connection, id, &workflow, now).await
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
            self.ticket(connection, id, &workflow, now).await
        })
    }

    pub async fn claim_next(
        &self,
        actor: &Actor<'_>,
        filter: &crate::NextFilter,
    ) -> BridgeResult<Option<Lease>> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let settings: Settings = self.configuration(connection, "settings").await?;
            self.check_wip(connection, actor, &settings, now).await?;
            let list = Filter {
                label: filter.label.clone(),
                parent: filter.parent.clone(),
                limit: Some(25),
                ..Default::default()
            };
            let candidates = self
                .list_in(connection, &workflow, now, &list, Some(actor.id))
                .await?;
            for mut ticket in candidates {
                if !filter.states.is_empty() && !filter.states.contains(&ticket.state) {
                    continue;
                }
                if ticket.holder.is_some() {
                    self.expire(connection, actor.id, now, &ticket, &workflow, &settings)
                        .await?;
                    ticket = self.ticket(connection, &ticket.id, &workflow, now).await?;
                }
                let Some(lane) = workflow.lane_by_queue(&ticket.state) else {
                    continue;
                };
                if !filter.states.is_empty() && !filter.states.contains(&ticket.state) {
                    continue;
                }
                if lane.distinct_from_previous && ticket.last_worker.as_deref() == Some(actor.id) {
                    continue;
                }
                return Ok(Some(
                    self.claim_in(
                        connection,
                        actor,
                        &ticket.id,
                        filter.ttl_ms,
                        now,
                        &workflow,
                        &settings,
                    )
                    .await?,
                ));
            }
            Ok(None)
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
                "UPDATE board_tickets SET assignee=$4 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(id)
            .bind(to)
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
            self.ticket(connection, id, &workflow, now).await
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
            if ticket.holder.is_none() {
                return Err(fail("not_held", format!("{id} is not held")).into());
            }
            self.clear_lease(connection,actor.id,now,&ticket,&crate::lease::release_state(&workflow,&ticket),
                actions::TICKET_FORCE_RELEASED,serde_json::json!({"previous_holder":ticket.holder,"previous_fence":ticket.fence,"reason":reason.trim()})).await?;
            self.ticket(connection, id, &workflow, now).await
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

fn lease(ticket: Ticket, settings: &Settings, already_held: bool) -> BridgeResult<Lease> {
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
        heartbeat_every_ms: crate::lease::beat_every(ttl),
        already_held,
        ticket,
    })
}
