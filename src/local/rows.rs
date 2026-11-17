//! Reading and writing rows of the standalone board: settings, workflow, tickets, dependencies and notes.

use devagotchi_contracts::BridgeResult;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{Dep, DepKind, Note, Settings, Ticket, Workflow, fail, store_error};

pub fn load_settings(connection: &Connection) -> BridgeResult<Settings> {
    let raw: String = connection
        .query_row(
            "SELECT value FROM settings WHERE key = 'settings'",
            [],
            |row| row.get(0),
        )
        .map_err(store_error)?;
    serde_json::from_str(&raw).map_err(|error| fail("board_settings", error.to_string()))
}

pub fn load_workflow(connection: &Connection) -> BridgeResult<Workflow> {
    let raw: String = connection
        .query_row(
            "SELECT value FROM settings WHERE key = 'workflow'",
            [],
            |row| row.get(0),
        )
        .map_err(store_error)?;
    serde_json::from_str(&raw).map_err(|error| fail("board_workflow", error.to_string()))
}

pub(crate) const TICKET_COLUMNS: &str = "id, kind, title, description, state, priority, labels, parent, assignee, holder,
    lease_id, lease_ttl_ms, lease_expires_at, heartbeat_at, fence, version, expiry_count, last_worker,
    progress, created_by, created_at, updated_at, closed_at, machine, repo, environment, agent, session,
    fingerprint, occurrences, last_seen_at";

pub(crate) fn ticket_from_row(row: &Row<'_>, now: i64) -> rusqlite::Result<Ticket> {
    let labels: String = row.get(6)?;
    let progress: Option<String> = row.get(18)?;
    let lease_expires_at: Option<i64> = row.get(12)?;
    Ok(Ticket {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        state: row.get(4)?,
        priority: row.get(5)?,
        labels: serde_json::from_str(&labels).unwrap_or_default(),
        parent: row.get(7)?,
        assignee: row.get(8)?,
        holder: row.get(9)?,
        lease_id: row.get(10)?,
        lease_ttl_ms: row.get(11)?,
        lease_expires_at,
        heartbeat_at: row.get(13)?,
        fence: row.get(14)?,
        version: row.get(15)?,
        expiry_count: row.get(16)?,
        last_worker: row.get(17)?,
        progress: progress.and_then(|p| serde_json::from_str(&p).ok()),
        created_by: row.get(19)?,
        created_at: row.get(20)?,
        updated_at: row.get(21)?,
        closed_at: row.get(22)?,
        machine: row.get(23)?,
        repo: row.get(24)?,
        environment: row.get(25)?,
        agent: row.get(26)?,
        session: row.get(27)?,
        fingerprint: row.get(28)?,
        occurrences: row.get(29)?,
        last_seen_at: row.get(30)?,
        blocked_by: Vec::new(),
        lease_expired: lease_expires_at.is_some_and(|at| at <= now),
    })
}

/// A ticket with its open blockers filled in.
pub fn read_ticket(
    connection: &Connection,
    workflow: &Workflow,
    id: &str,
    now: i64,
) -> BridgeResult<Ticket> {
    let mut ticket = connection
        .query_row(
            &format!("SELECT {TICKET_COLUMNS} FROM tickets WHERE id = ?1"),
            params![id],
            |row| ticket_from_row(row, now),
        )
        .optional()
        .map_err(store_error)?
        .ok_or_else(|| fail("not_found", format!("ticket {id} not found")))?;
    ticket.blocked_by = open_blockers(connection, workflow, id)?;
    Ok(ticket)
}

pub fn open_blockers(
    connection: &Connection,
    workflow: &Workflow,
    id: &str,
) -> BridgeResult<Vec<String>> {
    let mut statement = connection
        .prepare(
            "SELECT d.depends_on, t.state FROM deps d JOIN tickets t ON t.id = d.depends_on
             WHERE d.ticket = ?1 AND d.kind = 'blocks' ORDER BY d.depends_on",
        )
        .map_err(store_error)?;
    let rows = statement
        .query_map(params![id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(store_error)?;
    let mut open = Vec::new();
    for row in rows {
        let (depends_on, state) = row.map_err(store_error)?;
        if !workflow.is_done(&state) {
            open.push(depends_on);
        }
    }
    Ok(open)
}

/// Is there a `blocks` path from `from` to `to`? Used to refuse cycles.
pub(crate) fn reaches(connection: &Connection, from: &str, to: &str) -> BridgeResult<bool> {
    let found: bool = connection
        .query_row(
            "WITH RECURSIVE walk(id) AS (
               SELECT ?1
               UNION
               SELECT d.depends_on FROM deps d JOIN walk ON d.ticket = walk.id WHERE d.kind = 'blocks'
             ) SELECT EXISTS(SELECT 1 FROM walk WHERE id = ?2)",
            params![from, to],
            |row| row.get(0),
        )
        .map_err(store_error)?;
    Ok(found)
}

pub(crate) fn insert_dep(
    connection: &Connection,
    ticket: &str,
    depends_on: &str,
    kind: DepKind,
    actor: &str,
    now: i64,
) -> BridgeResult<()> {
    connection
        .execute(
            "INSERT OR IGNORE INTO deps (ticket, depends_on, kind, created_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ticket, depends_on, kind.as_str(), actor, now],
        )
        .map_err(store_error)?;
    Ok(())
}

pub(crate) fn read_deps(connection: &Connection, column: &str, id: &str) -> BridgeResult<Vec<Dep>> {
    let sql = match column {
        "ticket" => {
            "SELECT ticket, depends_on, kind FROM deps WHERE ticket = ?1 ORDER BY kind, depends_on"
        }
        _ => {
            "SELECT ticket, depends_on, kind FROM deps WHERE depends_on = ?1 ORDER BY kind, ticket"
        }
    };
    let mut statement = connection.prepare(sql).map_err(store_error)?;
    let rows = statement
        .query_map(params![id], |row| {
            let kind: String = row.get(2)?;
            Ok(Dep {
                ticket: row.get(0)?,
                depends_on: row.get(1)?,
                kind: DepKind::parse(&kind).unwrap_or(DepKind::Related),
            })
        })
        .map_err(store_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(store_error)
}

#[allow(clippy::too_many_arguments)]
pub fn insert_note(
    connection: &Connection,
    ticket: &str,
    author: &str,
    kind: &str,
    body: &str,
    to_principal: Option<&str>,
    fence: Option<i64>,
    now: i64,
) -> BridgeResult<i64> {
    connection
        .execute(
            "INSERT INTO notes (ticket, author, kind, body, to_principal, fence, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![ticket, author, kind, body, to_principal, fence, now],
        )
        .map_err(store_error)?;
    Ok(connection.last_insert_rowid())
}

pub(crate) fn note_from_row(row: &Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        seq: row.get(0)?,
        ticket: row.get(1)?,
        author: row.get(2)?,
        kind: row.get(3)?,
        body: row.get(4)?,
        to_principal: row.get(5)?,
        fence: row.get(6)?,
        created_at: row.get(7)?,
    })
}

pub(crate) fn read_note(connection: &Connection, seq: i64) -> BridgeResult<Note> {
    connection
        .query_row(
            "SELECT seq, ticket, author, kind, body, to_principal, fence, created_at FROM notes WHERE seq = ?1",
            params![seq],
            note_from_row,
        )
        .map_err(store_error)
}

pub(crate) fn read_notes(connection: &Connection, ticket: &str) -> BridgeResult<Vec<Note>> {
    let mut statement = connection
        .prepare(
            "SELECT seq, ticket, author, kind, body, to_principal, fence, created_at
             FROM notes WHERE ticket = ?1 ORDER BY seq",
        )
        .map_err(store_error)?;
    let rows = statement
        .query_map(params![ticket], note_from_row)
        .map_err(store_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(store_error)
}
