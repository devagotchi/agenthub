//! Claiming a named task or the next eligible task; replay never takes a second lease.
use super::*;
use crate::lease::projection as lease;
use crate::{Filter, chain::actions};

fn claim_action(workflow: &Workflow, ticket: &Ticket) -> String {
    let queue = workflow
        .lane_by_queue(&ticket.state)
        .or_else(|| workflow.lane_by_active(&ticket.state))
        .map(|lane| lane.queue.as_str())
        .unwrap_or(&ticket.state);
    format!("task.claim:{queue}")
}

impl HostedBoard {
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
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            let receipt: Receipt<(Lease, String)> =
                self.begin_mutation(connection, actor, "claim").await?;
            if let Receipt::Replay((lease, action)) = receipt {
                self.authorize(connection, actor, &action, &ticket.labels)
                    .await?;
                return Ok(lease);
            }
            let action = claim_action(&workflow, &ticket);
            let lease = self
                .claim_in(connection, actor, id, ttl_ms, now, &workflow, &settings)
                .await?;
            receipt
                .complete(connection, (lease, action))
                .await
                .map(|(lease, _)| lease)
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
        self.authorize(
            connection,
            actor,
            &claim_action(workflow, &ticket),
            &ticket.labels,
        )
        .await?;
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
        self.authorize(
            connection,
            actor,
            &format!("task.claim:{}", lane.queue),
            &ticket.labels,
        )
        .await?;
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

    /// Explicit principal limits count across boards; aliases without one use the board
    /// default. SERIALIZABLE protects this predicate against concurrent claims on other
    /// boards as well as this board's row lock. Claim and claim-next both ask this.
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
             WHERE tenant_id=$1 AND holder=$3 AND lease_expires_at>$4
               AND ($5 OR board_id=$2)",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(actor.id)
        .bind(now)
        .bind(actor.wip_limit.is_some())
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

    pub async fn claim_next(
        &self,
        actor: &Actor<'_>,
        filter: &crate::NextFilter,
    ) -> BridgeResult<Option<Lease>> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let settings: Settings = self.configuration(connection, "settings").await?;
            let receipt: Receipt<Option<(Lease, String)>> =
                self.begin_mutation(connection, actor, "claim_next").await?;
            if let Receipt::Replay(saved) = receipt {
                if let Some((lease, action)) = &saved {
                    let ticket = self
                        .ticket(connection, &lease.ticket.id, &workflow, now)
                        .await?;
                    self.authorize(connection, actor, action, &ticket.labels)
                        .await?;
                }
                return Ok(saved.map(|(lease, _)| lease));
            }
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
                if let Err(error) = self
                    .authorize(
                        connection,
                        actor,
                        &format!("task.claim:{}", lane.queue),
                        &ticket.labels,
                    )
                    .await
                {
                    match error {
                        Failed::Refused(error) if error.code == "policy_denied" => continue,
                        error => return Err(error),
                    }
                }
                let action = claim_action(&workflow, &ticket);
                let lease = self
                    .claim_in(
                        connection,
                        actor,
                        &ticket.id,
                        filter.ttl_ms,
                        now,
                        &workflow,
                        &settings,
                    )
                    .await?;
                return receipt
                    .complete(connection, Some((lease, action)))
                    .await
                    .map(|saved| saved.map(|(lease, _)| lease));
            }
            receipt
                .complete(connection, None)
                .await
                .map(|saved| saved.map(|(lease, _)| lease))
        })
    }
}
