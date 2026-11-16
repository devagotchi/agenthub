//! Listing tickets on the standalone board: the filter turned into SQL with numbered placeholders.

use devagotchi_contracts::BridgeResult;
use rusqlite::Connection;

use super::rows::TICKET_COLUMNS;
use crate::{Filter, Ticket, Workflow, clean_anchor, open_blockers, store_error, ticket_from_row};

/// Candidates in queue order, then the filters SQL cannot express (readiness needs
/// the workflow and the dependency graph), bounded by `limit`.
pub fn list_tickets(
    connection: &Connection,
    workflow: &Workflow,
    filter: &Filter,
    now: i64,
    ready_for: Option<&str>,
) -> BridgeResult<Vec<Ticket>> {
    let mut query = Query::new(format!("SELECT {TICKET_COLUMNS} FROM tickets WHERE 1 = 1"));
    if let Some(state) = &filter.state {
        query.and("state = ", Box::new(state.clone()));
    }
    if let Some(holder) = &filter.holder {
        query.and("holder = ", Box::new(holder.clone()));
    }
    if let Some(assignee) = &filter.assignee {
        query.and("assignee = ", Box::new(assignee.clone()));
    }
    if let Some(parent) = &filter.parent {
        query.and("parent = ", Box::new(parent.clone()));
    }
    if let Some(kind) = &filter.kind {
        query.and("kind = ", Box::new(kind.clone()));
    }
    if let Some(label) = &filter.label {
        query.and_wrapped(
            "EXISTS (SELECT 1 FROM json_each(tickets.labels) WHERE json_each.value = ",
            Box::new(label.clone()),
            ")",
        );
    }
    for (column, value) in filter.anchors.columns() {
        if let Some(value) = clean_anchor(value) {
            // `column` is one of six fixed names; the value is bound.
            query.and(&format!("{column} = "), Box::new(value));
        }
    }
    if let Some(text) = &filter.text {
        let pattern = format!(
            "%{}%",
            text.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        query.and_wrapped("(title LIKE ", Box::new(pattern.clone()), " ESCAPE '\\'");
        // Same pattern, second column: the clause is "... OR description LIKE ?n ESCAPE '\')".
        query.or_wrapped("description LIKE ", Box::new(pattern), " ESCAPE '\\')");
    }
    if filter.ready || ready_for.is_some() {
        query.and_wrapped(
            "(holder IS NULL OR lease_expires_at <= ",
            Box::new(now),
            ")",
        );
    }
    let (mut sql, args) = query.finish();
    sql.push_str(" ORDER BY (assignee IS NOT NULL) DESC, priority, created_at, id");
    let mut statement = connection.prepare(&sql).map_err(store_error)?;
    let params = rusqlite::params_from_iter(args.iter().map(|a| a.as_ref()));
    let rows = statement
        .query_map(params, |row| ticket_from_row(row, now))
        .map_err(store_error)?;
    let limit = filter.limit.unwrap_or(200).max(1);
    let mut out = Vec::new();
    for row in rows {
        let mut ticket = row.map_err(store_error)?;
        if filter.open_only && workflow.is_done(&ticket.state) {
            continue;
        }
        ticket.blocked_by = open_blockers(connection, workflow, &ticket.id)?;
        if filter.ready || ready_for.is_some() {
            if workflow.lane_by_queue(&ticket.state).is_none() && !ticket.lease_expired {
                continue;
            }
            if !ticket.blocked_by.is_empty() {
                continue;
            }
            if let Some(me) = ready_for {
                if ticket.assignee.as_deref().is_some_and(|a| a != me) {
                    continue;
                }
                let lane = workflow
                    .lane_by_queue(&ticket.state)
                    .or_else(|| workflow.lane_by_active(&ticket.state));
                if lane.is_some_and(|l| l.distinct_from_previous)
                    && ticket.last_worker.as_deref() == Some(me)
                {
                    continue;
                }
            }
        }
        out.push(ticket);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// SQL assembled clause by clause with numbered placeholders, so a filter never
/// reaches the statement as text.
pub(crate) struct Query {
    pub(crate) sql: String,
    pub(crate) args: Vec<Box<dyn rusqlite::ToSql>>,
}

impl Query {
    pub(crate) fn new(sql: String) -> Self {
        Self {
            sql,
            args: Vec::new(),
        }
    }

    pub(crate) fn and(&mut self, clause: &str, value: Box<dyn rusqlite::ToSql>) {
        self.and_wrapped(clause, value, "");
    }

    pub(crate) fn and_wrapped(
        &mut self,
        before: &str,
        value: Box<dyn rusqlite::ToSql>,
        after: &str,
    ) {
        self.args.push(value);
        self.sql
            .push_str(&format!(" AND {before}?{}{after}", self.args.len()));
    }

    pub(crate) fn or_wrapped(
        &mut self,
        before: &str,
        value: Box<dyn rusqlite::ToSql>,
        after: &str,
    ) {
        self.args.push(value);
        self.sql
            .push_str(&format!(" OR {before}?{}{after}", self.args.len()));
    }

    pub(crate) fn finish(self) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
        (self.sql, self.args)
    }
}
