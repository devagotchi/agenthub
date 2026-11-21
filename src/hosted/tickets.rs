//! Making and changing a ticket: the welcome ticket, create (with its import source, once
//! per board), update, delegate and undo-close. Reading one whole is `reads.rs`.

use super::edits::unheld_edit;
use super::*;
use crate::chain::actions;
use crate::{TicketProvenance, TicketSource, provenance::request_digest};

impl HostedBoard {
    pub async fn welcome_if_new(
        &self,
        actor: &Actor<'_>,
        title: &str,
        steps: &str,
    ) -> BridgeResult<Option<String>> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM board_tickets WHERE tenant_id=$1 AND board_id=$2",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .fetch_one(&mut *connection)
            .await?;
            if count > 0 {
                return Ok(None);
            }
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let id = self.fresh_id(connection).await?;
            sqlx::query(
                "INSERT INTO board_tickets (tenant_id,board_id,id,kind,title,description,state,priority,labels,created_by,created_at,updated_at)
                 VALUES ($1,$2,$3,'task',$4,$5,$6,1,'[\"getting-started\"]',$7,$8,$8)",
            ).bind(&self.tenant).bind(&self.project).bind(&id).bind(title).bind(steps)
                .bind(&workflow.attention_state).bind(actor.id).bind(now).execute(&mut *connection).await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::BOARD_WELCOMED,
                Some(&id),
                None,
                serde_json::json!({"title":title}),
            )
            .await?;
            Ok(Some(id))
        })
    }

    pub async fn create(&self, actor: &Actor<'_>, new: &NewTicket) -> BridgeResult<Ticket> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            self.authorize(connection, actor, "task.create", &clean_labels(&new.labels))
                .await?;
            let receipt: Receipt<Ticket> = self
                .begin_mutation(connection, actor, "create_task")
                .await?;
            if let Receipt::Replay(result) = receipt {
                let workflow = self.configuration(connection, "workflow").await?;
                let current = self.ticket(connection, &result.id, &workflow, now).await?;
                self.authorize(connection, actor, "task.create", &current.labels)
                    .await?;
                return Ok(result);
            }
            let result = self.create_in(connection, actor, new, now).await?;
            receipt.complete(connection, result).await
        })
    }

    /// Make a ticket in the caller's transaction. With a source, the board's ticket for it
    /// when there is one: as it is now when the inputs are the same, `source_conflict` when
    /// they are not, either way before anything is written. A concurrent creator of the same
    /// source collides on the source's key, runs again, and finds the winner's ticket.
    pub(super) async fn create_in(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        new: &NewTicket,
        now: i64,
    ) -> DbResult<Ticket> {
        let title = new.title.trim();
        if title.is_empty() || title.chars().count() > 300 {
            return Err(invalid("title must be 1 to 300 characters").into());
        }
        let kind = new.kind.as_deref().unwrap_or("task");
        if !matches!(kind, "task" | "epic" | "message") {
            return Err(invalid("kind must be task, epic or message").into());
        }
        let occurrences = new.occurrences.unwrap_or(1);
        if occurrences < 1 {
            return Err(invalid("occurrences must be at least 1").into());
        }
        let labels = clean_labels(&new.labels);
        self.authorize(connection, actor, "task.create", &labels)
            .await?;
        let environment = clean_anchor(&new.anchors.environment);
        let fingerprint = clean_anchor(&new.anchors.fingerprint);
        let workflow: Workflow = self.configuration(connection, "workflow").await?;
        let request_sha256 = match &new.source {
            Some(source) => {
                source.validate()?;
                let digest = request_digest(
                    new,
                    title,
                    kind,
                    &labels,
                    environment.as_deref(),
                    fingerprint.as_deref(),
                );
                if let Some(id) = self.sourced(connection, source, &digest).await? {
                    return self.ticket(connection, &id, &workflow, now).await;
                }
                Some(digest)
            }
            None => None,
        };
        let settings: Settings = self.configuration(connection, "settings").await?;
        check_environment(&settings, environment.as_deref())?;
        // No repo given: the board's, so every face that creates gets it (rule 1). Anchors
        // are not part of `request_sha256`, so a sourced repeat is unaffected.
        let repo = clean_anchor(&new.anchors.repo).or_else(|| settings.repo.clone());
        let id = if let Some(parent) = &new.parent {
            let parent_ticket = self.ticket(connection, parent, &workflow, now).await?;
            if workflow.is_done(&parent_ticket.state) {
                return Err(fail("parent_closed", format!("{parent} is closed")).into());
            }
            let next: i64 = sqlx::query_scalar(
                "UPDATE board_tickets SET child_seq=child_seq+1
                     WHERE tenant_id=$1 AND board_id=$2 AND id=$3 RETURNING child_seq",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(parent)
            .fetch_one(&mut *connection)
            .await?;
            format!("{parent}.{next}")
        } else {
            self.fresh_id(connection).await?
        };
        if let Some(from) = &new.discovered_from {
            self.ticket(connection, from, &workflow, now).await?;
        }
        let dependencies: std::collections::BTreeSet<_> = new.depends_on.iter().collect();
        if dependencies.len() > 100 {
            return Err(invalid("a new ticket may depend on at most 100 tickets").into());
        }
        for dependency in &dependencies {
            self.ticket(connection, dependency, &workflow, now).await?;
        }
        sqlx::query(
                "INSERT INTO board_tickets
                 (tenant_id,board_id,id,kind,title,description,state,priority,labels,parent,assignee,
                  created_by,created_at,updated_at,machine,repo,environment,agent,session,fingerprint,occurrences,last_seen_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$13,$14,$15,$16,$17,$18,$19,$20,$21)",
            ).bind(&self.tenant).bind(&self.project).bind(&id).bind(kind).bind(title)
                .bind(&new.description).bind(&workflow.initial_state)
                .bind(new.priority.unwrap_or(settings.default_priority))
                .bind(serde_json::to_string(&labels).expect("labels json"))
                .bind(&new.parent).bind(&new.assignee).bind(actor.id).bind(now)
                .bind(clean_anchor(&new.anchors.machine)).bind(&repo)
                .bind(environment).bind(clean_anchor(&new.anchors.agent))
                .bind(clean_anchor(&new.anchors.session)).bind(fingerprint)
                .bind(occurrences).bind(new.last_seen_at).execute(&mut *connection).await?;
        for (target, kind) in [
            (&new.parent, "parent"),
            (&new.discovered_from, "discovered-from"),
        ] {
            if let Some(target) = target {
                sqlx::query(
                        "INSERT INTO board_deps (tenant_id,board_id,ticket,depends_on,kind,created_by,created_at)
                         VALUES ($1,$2,$3,$4,$5,$6,$7)",
                    ).bind(&self.tenant).bind(&self.project).bind(&id).bind(target).bind(kind)
                        .bind(actor.id).bind(now).execute(&mut *connection).await?;
            }
        }
        let mut data =
            serde_json::json!({"title":title,"state":workflow.initial_state,"parent":new.parent});
        for dependency in &dependencies {
            sqlx::query(
                "INSERT INTO board_deps (tenant_id,board_id,ticket,depends_on,kind,created_by,created_at)
                 VALUES ($1,$2,$3,$4,'blocks',$5,$6)",
            ).bind(&self.tenant).bind(&self.project).bind(&id).bind(dependency.as_str())
                .bind(actor.id).bind(now).execute(&mut *connection).await?;
        }
        if !dependencies.is_empty() {
            data["depends_on"] = serde_json::json!(dependencies);
        }
        if let (Some(source), Some(request_sha256)) = (&new.source, request_sha256) {
            let provenance = TicketProvenance {
                source: source.clone(),
                recorded_by: actor.id.to_string(),
                recorded_at: now,
                request_sha256,
            };
            sqlx::query(
                "INSERT INTO board_sources (tenant_id,board_id,ticket,path,item,revision,content_sha256,
                  recorded_by,recorded_at,request_sha256) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(&id)
            .bind(&source.path)
            .bind(&source.item)
            .bind(&source.revision)
            .bind(&source.content_sha256)
            .bind(&provenance.recorded_by)
            .bind(now)
            .bind(&provenance.request_sha256)
            .execute(&mut *connection)
            .await?;
            data["provenance"] = serde_json::to_value(&provenance).expect("provenance json");
        }
        self.event(
            connection,
            actor.id,
            now,
            actions::TICKET_CREATED,
            Some(&id),
            None,
            data,
        )
        .await?;
        self.ticket(connection, &id, &workflow, now).await
    }

    /// The ticket already made from `source`, if any, when `digest` is the inputs it was made
    /// with; `source_conflict` when they differ.
    async fn sourced(
        &self,
        connection: &mut AnyConnection,
        source: &TicketSource,
        digest: &str,
    ) -> DbResult<Option<String>> {
        let row = sqlx::query(
            "SELECT ticket,request_sha256 FROM board_sources
             WHERE tenant_id=$1 AND board_id=$2 AND path=$3 AND item=$4",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(&source.path)
        .bind(&source.item)
        .fetch_optional(&mut *connection)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let id: String = row.try_get("ticket")?;
        if row.try_get::<String, _>("request_sha256")? != digest {
            return Err(fail(
                "source_conflict",
                format!(
                    "source {}#{} already belongs to {id}, made with different inputs; reconcile that ticket",
                    source.path, source.item
                ),
            )
            .into());
        }
        Ok(Some(id))
    }

    /// Reopen a ticket closed less than `UNDO_MS` ago by the database's clock: back to
    /// `from` when the workflow goes there, else to its initial state. `undo_too_late` once
    /// the window has passed; `transition_not_allowed` when the workflow has neither way back.
    pub async fn undo_close(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        from: &str,
    ) -> BridgeResult<Ticket> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let current = self.ticket(connection, id, &workflow, now).await?;
            if current
                .closed_at
                .is_none_or(|closed| now >= closed + crate::UNDO_MS)
            {
                return Err(fail(
                    "undo_too_late",
                    format!("{id} was closed more than {}s ago", crate::UNDO_MS / 1000),
                )
                .into());
            }
            unheld_edit(&current, version)?;
            let to = if workflow.allows(&current.state, from) {
                from.to_string()
            } else {
                workflow.initial_state.clone()
            };
            if !workflow.allows(&current.state, &to) {
                return Err(fail(
                    "transition_not_allowed",
                    format!(
                        "this board's workflow ({}) has no transition {} -> {}; add {{\"from\": \"{}\", \"to\": \"{}\"}} to its transitions to undo here",
                        workflow.id, current.state, to, current.state, to
                    ),
                )
                .into());
            }
            crate::check_transition(&workflow, &current, &to, actor.id)?;
            self.authorize(
                connection,
                actor,
                &format!("task.transition:{}->{to}", current.state),
                &current.labels,
            )
            .await?;
            sqlx::query(
                "UPDATE board_tickets SET state=$4,closed_at=NULL,version=version+1,updated_at=$5
                 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(id)
            .bind(&to)
            .bind(now)
            .execute(&mut *connection)
            .await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::TICKET_UPDATED,
                Some(id),
                None,
                serde_json::json!({"from": current.state, "to": to, "undo": true}),
            )
            .await?;
            self.ticket(connection, id, &workflow, now).await
        })
    }
}
