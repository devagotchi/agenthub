//! Making a ticket and reading it whole: create (with its import source, once per board) and
//! the full view with its dependencies, children, notes and provenance.

use super::*;
use crate::{TicketProvenance, TicketSource, provenance::request_digest};

impl HostedBoard {
    pub async fn create(&self, actor: &Actor<'_>, new: &NewTicket) -> BridgeResult<Ticket> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            self.create_in(connection, actor, new, now).await
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
                .bind(clean_anchor(&new.anchors.machine)).bind(clean_anchor(&new.anchors.repo))
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
            "ticket.created",
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

    async fn provenance(
        &self,
        connection: &mut AnyConnection,
        id: &str,
    ) -> DbResult<Option<TicketProvenance>> {
        let row = sqlx::query(
            "SELECT path,item,revision,content_sha256,recorded_by,recorded_at,request_sha256
             FROM board_sources WHERE tenant_id=$1 AND board_id=$2 AND ticket=$3",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(id)
        .fetch_optional(&mut *connection)
        .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(TicketProvenance {
            source: TicketSource {
                path: row.try_get("path")?,
                item: row.try_get("item")?,
                revision: row.try_get("revision")?,
                content_sha256: row.try_get("content_sha256")?,
            },
            recorded_by: row.try_get("recorded_by")?,
            recorded_at: row.try_get("recorded_at")?,
            request_sha256: row.try_get("request_sha256")?,
        }))
    }

    pub async fn view(&self, id: &str) -> BridgeResult<crate::TicketView> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            let deps = self.deps(connection, id, false).await?;
            let dependents = self.deps(connection, id, true).await?;
            let children=sqlx::query_scalar(
                "SELECT id FROM board_tickets WHERE tenant_id=$1 AND board_id=$2 AND parent=$3 ORDER BY created_at,id COLLATE \"C\"",
            ).bind(&self.tenant).bind(&self.project).bind(id).fetch_all(&mut *connection).await?;
            let texts:Vec<String>=sqlx::query_scalar(
                "SELECT row_to_json(n)::text FROM board_notes n WHERE tenant_id=$1 AND board_id=$2 AND ticket=$3 ORDER BY seq",
            ).bind(&self.tenant).bind(&self.project).bind(id).fetch_all(&mut *connection).await?;
            let notes = texts
                .iter()
                .map(|text| decode(text))
                .collect::<BridgeResult<Vec<Note>>>()?;
            let provenance = self.provenance(connection, id).await?;
            Ok(crate::TicketView {
                ticket,
                deps,
                dependents,
                children,
                notes,
                provenance,
            })
        })
    }
}
