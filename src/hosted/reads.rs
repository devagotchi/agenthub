//! Reading a board: one ticket whole (dependencies, children, notes, provenance), many at
//! once (a filter, the ready queue), and verifying the chain.

use super::*;
use crate::{ChainStatus, Filter, TicketProvenance, TicketSource};

impl HostedBoard {
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

    pub(super) async fn list_in(
        &self,
        connection: &mut AnyConnection,
        workflow: &Workflow,
        now: i64,
        filter: &Filter,
        ready_for: Option<&str>,
    ) -> DbResult<Vec<Ticket>> {
        let text = filter
            .text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        // Keep the existing literal wildcard contract while restoring web-search terms.
        let web_search = text.is_some_and(|text| !text.contains(['%', '_', '\\']));
        let pattern = text.map(|text| {
            format!(
                "%{}%",
                text.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM board_tickets
             WHERE tenant_id=$1 AND board_id=$2
               AND ($3::text IS NULL OR state=$3)
               AND ($4::text IS NULL OR holder=$4)
               AND ($5::text IS NULL OR assignee=$5)
               AND ($6::text IS NULL OR parent=$6)
               AND ($7::text IS NULL OR kind=$7)
               AND ($8::text IS NULL OR jsonb_exists(labels::jsonb,$8))
               AND ($9::text IS NULL OR title ILIKE $9 ESCAPE '\\' OR description ILIKE $9 ESCAPE '\\'
                    OR id=$16
                    OR ($17::boolean AND to_tsvector('simple', title || ' ' || description)
                        @@ websearch_to_tsquery('simple', $16))
                    OR EXISTS (SELECT 1 FROM board_notes n
                        WHERE n.tenant_id=$1 AND n.board_id=$2 AND n.ticket=board_tickets.id
                          AND n.body ILIKE $9 ESCAPE '\\'))
               AND ($10::text IS NULL OR machine=$10)
               AND ($11::text IS NULL OR repo=$11)
               AND ($12::text IS NULL OR environment=$12)
               AND ($13::text IS NULL OR agent=$13)
               AND ($14::text IS NULL OR session=$14)
               AND ($15::text IS NULL OR fingerprint=$15)
             ORDER BY (assignee IS NOT NULL) DESC,priority,created_at,id COLLATE \"C\"",
        ).bind(&self.tenant).bind(&self.project).bind(&filter.state).bind(&filter.holder)
            .bind(&filter.assignee).bind(&filter.parent).bind(&filter.kind).bind(&filter.label).bind(pattern)
            .bind(clean_anchor(&filter.anchors.machine)).bind(clean_anchor(&filter.anchors.repo))
            .bind(clean_anchor(&filter.anchors.environment)).bind(clean_anchor(&filter.anchors.agent))
            .bind(clean_anchor(&filter.anchors.session)).bind(clean_anchor(&filter.anchors.fingerprint))
            .bind(text).bind(web_search)
            .fetch_all(&mut *connection).await?;
        let mut tickets = Vec::new();
        let limit = filter.limit.unwrap_or(200).max(1);
        for id in ids {
            let ticket = self.ticket(connection, &id, workflow, now).await?;
            if filter.open_only && workflow.is_done(&ticket.state) {
                continue;
            }
            if filter.ready || ready_for.is_some() {
                if ticket.holder.is_some() && !ticket.lease_expired {
                    continue;
                }
                if workflow.lane_by_queue(&ticket.state).is_none() && !ticket.lease_expired {
                    continue;
                }
                if !ticket.blocked_by.is_empty() {
                    continue;
                }
                if let Some(actor) = ready_for {
                    if ticket
                        .assignee
                        .as_deref()
                        .is_some_and(|assigned| assigned != actor)
                    {
                        continue;
                    }
                    let lane = workflow
                        .lane_by_queue(&ticket.state)
                        .or_else(|| workflow.lane_by_active(&ticket.state));
                    if lane.is_some_and(|lane| lane.distinct_from_previous)
                        && ticket.last_worker.as_deref() == Some(actor)
                    {
                        continue;
                    }
                }
            }
            tickets.push(ticket);
            if tickets.len() >= limit {
                break;
            }
        }
        Ok(tickets)
    }

    pub async fn list(&self, filter: &Filter) -> BridgeResult<Vec<Ticket>> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            self.list_in(connection, &workflow, now, filter, None).await
        })
    }

    pub async fn ready(&self, actor: &Actor<'_>, limit: usize) -> BridgeResult<Vec<Ticket>> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            self.list_in(
                connection,
                &workflow,
                now,
                &Filter {
                    ready: true,
                    limit: Some(limit),
                    ..Default::default()
                },
                Some(actor.id),
            )
            .await
        })
    }

    pub async fn verify_chain(&self) -> BridgeResult<ChainStatus> {
        let events = self.events().await?;
        let mut previous = GENESIS.to_string();
        let mut broken = None;
        for (index, event) in events.iter().enumerate() {
            if broken.is_none()
                && (event.seq != index as i64 + 1
                    || event.prev_hash != previous
                    || event.hash != event_hash(&previous, event))
            {
                broken = Some(event.seq);
            }
            previous = event.hash.clone();
        }
        Ok(ChainStatus {
            entries: events.len() as u64,
            ok: broken.is_none(),
            head: previous,
            first_broken: broken,
        })
    }
}
