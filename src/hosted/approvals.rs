//! Approvals in Postgres: asking (decided at once when a live standing authorization covers
//! the step), a person's decision (optionally allowing the same step ahead of time), and the
//! reads the approvals page shows. Standing authorizations themselves are `standing.rs`.
//! Each intent is one transaction on the database clock
//! (`.ai/patterns/board-operations.md`, rules 1 to 3).

use super::standing::{checked_days, standing_id};
use super::*;
use crate::approval::{
    ApprovalStatus, Ask, DAY_MS, KIND, Standing, Verdict, checked, label_value, landing, text,
};
use crate::chain::actions;

impl HostedBoard {
    /// Ask a person to approve one step. With a kind (and a target), a live standing
    /// authorization that covers the step decides it at once, in the same transaction, and
    /// comes back with it.
    pub async fn ask(
        &self,
        actor: &Actor<'_>,
        ask: &Ask<'_>,
    ) -> BridgeResult<(ApprovalStatus, Option<Standing>)> {
        let kind = ask.kind.map(|kind| checked("kind", kind)).transpose()?;
        let target = ask
            .target
            .map(|target| checked("target", target))
            .transpose()?;
        if target.is_some() && kind.is_none() {
            return Err(invalid(
                "a target goes with a kind: name the kind of step too",
            ));
        }
        let mut labels = vec![KIND.to_string()];
        labels.extend(kind.iter().map(|kind| format!("kind:{kind}")));
        labels.extend(target.iter().map(|target| format!("target:{target}")));
        let labels = serde_json::to_string(&labels).expect("labels json");
        let title = text("title", ask.title, 300, true)?;
        let what = text("what", ask.what, 4000, true)?;
        let why = text("why", ask.why, 4000, false)?;
        let undo = text("undo", ask.undo, 4000, false)?;
        let description = format!("What: {what}\n\nWhy: {why}\n\nUndo: {undo}");
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let id = self.fresh_id(connection).await?;
            sqlx::query(
                "INSERT INTO board_tickets
                 (tenant_id,board_id,id,kind,title,description,state,priority,labels,created_by,created_at,updated_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,1,$8,$9,$10,$10)",
            ).bind(&self.tenant).bind(&self.project).bind(&id).bind(KIND).bind(&title)
                .bind(&description).bind(&workflow.attention_state).bind(&labels).bind(actor.id)
                .bind(now).execute(&mut *connection).await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::APPROVAL_REQUESTED,
                Some(&id),
                None,
                serde_json::json!({"title":title}),
            )
            .await?;
            let status = self.approval_in(connection, &id, &workflow, now).await?;
            let (Some(kind), Some(what)) = (&kind, status.what().map(str::to_string)) else {
                return Ok((status, None));
            };
            let settings: Settings = self.configuration(connection, "settings").await?;
            let Some(standing) = settings
                .standing
                .into_iter()
                .find(|s| s.live(now) && s.covers(kind, target.as_deref(), &what))
            else {
                return Ok((status, None));
            };
            let by = format!("standing:{}", standing.id);
            let note = format!(
                "Approved at once: {} approved {} ahead of time until {}.",
                standing.granted_by,
                match &standing.target {
                    Some(target) => format!("{} on {target}", standing.kind),
                    None => standing.kind.clone(),
                },
                crate::utc((standing.expires_at / 1000).max(0) as u64)
            );
            let standing_actor = Actor {
                id: &by,
                human: true,
                wip_limit: None,
            };
            let decided = self
                .decide_in(
                    connection,
                    &standing_actor,
                    &status.id,
                    status.version,
                    Verdict::Approved,
                    &note,
                    &workflow,
                    now,
                )
                .await?;
            Ok((decided, Some(standing)))
        })
    }

    async fn approval_in(
        &self,
        connection: &mut AnyConnection,
        id: &str,
        workflow: &Workflow,
        now: i64,
    ) -> DbResult<ApprovalStatus> {
        let ticket = self.ticket(connection, id, workflow, now).await?;
        if ticket.kind != KIND {
            return Err(fail("not_an_approval", format!("{id} is not an approval")).into());
        }
        let decision = sqlx::query(
            "SELECT actor,at,data FROM board_events WHERE tenant_id=$1 AND board_id=$2 AND ticket=$3
             AND action=$4 ORDER BY seq DESC LIMIT 1",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(id)
        .bind(actions::APPROVAL_DECIDED)
        .fetch_optional(&mut *connection)
        .await?;
        let (status, decided_by, decided_at, note) = match decision {
            Some(row) => {
                let data: serde_json::Value = decode(&row.try_get::<String, _>("data")?)?;
                let verdict = data["verdict"]
                    .as_str()
                    .filter(|v| matches!(*v, "approved" | "denied"))
                    .ok_or_else(|| fail("board_damaged", "invalid approval verdict"))?;
                (
                    verdict.into(),
                    Some(row.try_get("actor")?),
                    Some(row.try_get("at")?),
                    data["note"]
                        .as_str()
                        .filter(|note| !note.is_empty())
                        .map(str::to_string),
                )
            }
            None => ("pending".into(), None, None, None),
        };
        Ok(ApprovalStatus {
            id: ticket.id,
            title: ticket.title,
            status,
            description: ticket.description,
            requested_by: ticket.created_by,
            requested_at: ticket.created_at,
            version: ticket.version,
            decided_by,
            decided_at,
            note,
            kind: label_value(&ticket.labels, "kind:"),
            target: label_value(&ticket.labels, "target:"),
        })
    }

    pub async fn approval_status(&self, id: &str) -> BridgeResult<ApprovalStatus> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            self.approval_in(connection, id, &workflow, now).await
        })
    }

    pub async fn pending_approvals(&self) -> BridgeResult<Vec<ApprovalStatus>> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let ids: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM board_tickets WHERE tenant_id=$1 AND board_id=$2 AND kind=$4
                 AND state=$3 ORDER BY created_at,id COLLATE \"C\"",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(&workflow.attention_state)
            .bind(KIND)
            .fetch_all(&mut *connection)
            .await?;
            let mut approvals = Vec::with_capacity(ids.len());
            for id in ids {
                approvals.push(self.approval_in(connection, &id, &workflow, now).await?);
            }
            Ok(approvals)
        })
    }

    /// A person approves or denies `id` as they saw it at `version`.
    pub async fn decide_approval(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        verdict: Verdict,
        note: &str,
    ) -> BridgeResult<ApprovalStatus> {
        if !actor.human {
            return Err(fail("humans_only", "only a person can approve or deny"));
        }
        let note = text("note", note, 2000, false)?;
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            self.decide_in(
                connection, actor, id, version, verdict, &note, &workflow, now,
            )
            .await
        })
    }

    /// "Approve and allow": a person approves `id` and, when it names a kind and reads as one
    /// exact command, approves that same step ahead of time for `days`, in one transaction
    /// (`approval.decided`, then `approval.standing_granted`). A grant that cannot be made
    /// (`too_many`) leaves the approval undecided.
    pub async fn decide_and_allow(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        note: &str,
        days: u32,
    ) -> BridgeResult<(ApprovalStatus, Option<Standing>)> {
        if !actor.human {
            return Err(fail("humans_only", "only a person can approve or deny"));
        }
        let note = text("note", note, 2000, false)?;
        let days = checked_days(days)?;
        let standing_id = standing_id();
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let decided = self
                .decide_in(
                    connection,
                    actor,
                    id,
                    version,
                    Verdict::Approved,
                    &note,
                    &workflow,
                    now,
                )
                .await?;
            let (Some(kind), Some(what)) = (decided.kind.clone(), decided.what()) else {
                return Ok((decided, None));
            };
            let made = Standing {
                id: standing_id.clone(),
                kind,
                target: decided.target.clone(),
                what: what.to_string(),
                granted_by: actor.id.to_string(),
                granted_at: now,
                expires_at: now + i64::from(days) * DAY_MS,
                revoked_at: None,
                revoked_by: None,
            };
            let standing = self.grant_in(connection, actor, now, made).await?;
            Ok((decided, Some(standing)))
        })
    }

    /// The decision itself, in the caller's transaction: refused unless `id` is an approval
    /// still waiting at `version`.
    #[allow(clippy::too_many_arguments)]
    async fn decide_in(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        verdict: Verdict,
        note: &str,
        workflow: &Workflow,
        now: i64,
    ) -> DbResult<ApprovalStatus> {
        let ticket = self.ticket(connection, id, workflow, now).await?;
        if ticket.kind != KIND {
            return Err(fail("not_an_approval", format!("{id} is not an approval")).into());
        }
        if ticket.state != workflow.attention_state {
            return Err(fail("already_decided", format!("{id} is already decided")).into());
        }
        if ticket.version != version {
            return Err(fail(
                "approval_changed",
                format!("{id} changed since you saw it; look again before deciding"),
            )
            .into());
        }
        let to = landing(workflow, verdict)?;
        sqlx::query(
            "UPDATE board_tickets SET state=$4,closed_at=$5,version=version+1,updated_at=$5
             WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(id)
        .bind(&to)
        .bind(now)
        .execute(&mut *connection)
        .await?;
        if !note.is_empty() {
            self.insert_note(connection, actor.id, now, id, "decision", note, None, None)
                .await?;
        }
        self.event(
            connection,
            actor.id,
            now,
            actions::APPROVAL_DECIDED,
            Some(id),
            None,
            serde_json::json!({"verdict":verdict,"note":note,"to":to}),
        )
        .await?;
        self.approval_in(connection, id, workflow, now).await
    }
}
