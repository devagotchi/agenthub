//! What a host needs to know about the boards in a tenant without opening one: the names an
//! owner holds, each board with its owner, whether a board exists, and when each board's
//! first lease lapses. The hub's board registry and its signal sink ask these instead of
//! running their own SQL on `board_*` tables, so a change to the tables changes them here.

use devagotchi_contracts::BridgeResult;
use devagotchi_store::{Store, transaction};

/// Maximum claims in one roster page and boards in its authorized visibility set.
pub const MAX_WORKLOAD_CLAIMS: usize = 100;
pub const MAX_WORKLOAD_BOARDS: usize = 1000;

/// A stable key within a principal's live claims, ordered by board then ticket id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCursor {
    pub board_id: String,
    pub ticket_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalClaim {
    pub cursor: ClaimCursor,
    pub title: String,
    pub lease_expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalWorkload {
    /// Database time, in milliseconds, shared by the total and visible claims.
    pub as_of: i64,
    /// All live claims for this identity in the tenant, including inaccessible boards.
    pub total: i64,
    pub claims: Vec<PrincipalClaim>,
    /// The last returned claim, only when another visible claim exists in this snapshot.
    pub next: Option<ClaimCursor>,
}

/// Read a verified principal's workload without revealing inaccessible board details.
/// The host must authenticate `principal`, authorize disclosure of its total, and supply
/// only board ids this viewer may read. A cursor never grants board access. Subsequent
/// pages are fresh snapshots: claims can expire or move between requests.
pub async fn principal_workload(
    store: &Store,
    tenant: &str,
    principal: &str,
    visible_boards: &[String],
    after: Option<&ClaimCursor>,
    limit: usize,
) -> BridgeResult<PrincipalWorkload> {
    if !devagotchi_store::valid_tenant_id(tenant)
        || principal.is_empty()
        || principal.len() > 256
        || visible_boards.len() > MAX_WORKLOAD_BOARDS
        || visible_boards
            .iter()
            .any(|id| id.is_empty() || id.len() > 256)
        || !(1..=MAX_WORKLOAD_CLAIMS).contains(&limit)
        || after.is_some_and(|key| {
            key.board_id.is_empty()
                || key.board_id.len() > 256
                || key.ticket_id.is_empty()
                || key.ticket_id.len() > 256
        })
    {
        return Err(crate::invalid("invalid principal workload page bounds"));
    }
    let visible = serde_json::to_string(visible_boards).expect("string list json");
    transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        let now = super::HostedBoard::server_now(connection).await?;
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM board_tickets
             WHERE tenant_id=$1 AND holder=$2 AND lease_expires_at>$3",
        )
        .bind(tenant)
        .bind(principal)
        .bind(now)
        .fetch_one(&mut *connection)
        .await?;
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT board_id,id,title,lease_expires_at FROM board_tickets
             WHERE tenant_id=$1 AND holder=$2 AND lease_expires_at>$3
               AND board_id IN (SELECT jsonb_array_elements_text($4::jsonb))
               AND (board_id COLLATE \"C\",id COLLATE \"C\") > ($5,$6)
             ORDER BY board_id COLLATE \"C\",id COLLATE \"C\" LIMIT $7",
        )
        .bind(tenant)
        .bind(principal)
        .bind(now)
        .bind(&visible)
        .bind(after.map_or("", |key| key.board_id.as_str()))
        .bind(after.map_or("", |key| key.ticket_id.as_str()))
        .bind((limit + 1) as i64)
        .fetch_all(&mut *connection)
        .await?;
        let more = rows.len() > limit;
        let claims: Vec<_> = rows
            .into_iter()
            .take(limit)
            .map(
                |(board_id, ticket_id, title, lease_expires_at)| PrincipalClaim {
                    cursor: ClaimCursor {
                        board_id,
                        ticket_id,
                    },
                    title,
                    lease_expires_at,
                },
            )
            .collect();
        let next = if more {
            claims.last().map(|claim| claim.cursor.clone())
        } else {
            None
        };
        Ok(PrincipalWorkload {
            as_of: now,
            total,
            claims,
            next,
        })
    })
}

/// One board in a tenant, as `board_projects` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardRow {
    /// The board's id: its configured name, or `b-` and the owner's hash.
    pub id: String,
    pub owner: Option<String>,
    /// The name its owner gave it.
    pub project_name: Option<String>,
}

/// The board names `owner` holds in `tenant`, in byte order.
pub async fn names(store: &Store, tenant: &str, owner: &str) -> BridgeResult<Vec<String>> {
    transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        Ok(sqlx::query_scalar(
            "SELECT project_name FROM board_projects WHERE tenant_id=$1 AND owner_id=$2 AND retired_at IS NULL
             ORDER BY project_name COLLATE \"C\"",
        )
        .bind(tenant)
        .bind(owner)
        .fetch_all(&mut *connection)
        .await?)
    })
}

/// Every board in `tenant`, by id.
pub async fn boards(store: &Store, tenant: &str) -> BridgeResult<Vec<BoardRow>> {
    let rows: Vec<(String, Option<String>, Option<String>)> = transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        Ok(sqlx::query_as(
            "SELECT id, owner_id, project_name FROM board_projects WHERE tenant_id=$1 AND retired_at IS NULL ORDER BY id",
        )
        .bind(tenant)
        .fetch_all(&mut *connection)
        .await?)
    })?;
    Ok(rows
        .into_iter()
        .map(|(id, owner, project_name)| BoardRow {
            id,
            owner,
            project_name,
        })
        .collect())
}

/// Whether `owner` holds a board named `project` in `tenant`.
pub async fn has_board(
    store: &Store,
    tenant: &str,
    owner: &str,
    project: &str,
) -> BridgeResult<bool> {
    transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM board_projects WHERE tenant_id=$1 AND owner_id=$2 AND project_name=$3 AND retired_at IS NULL",
        )
        .bind(tenant)
        .bind(owner)
        .bind(project)
        .fetch_one(&mut *connection)
        .await?;
        Ok(count == 1)
    })
}

/// For each board in `tenant` with a lease held, how many milliseconds until its first lease
/// lapses, on the database's clock (zero or less: one has lapsed).
pub async fn lease_delays(store: &Store, tenant: &str) -> BridgeResult<Vec<(String, i64)>> {
    transaction!(store, |connection| {
        store.scope(connection, tenant).await?;
        Ok(sqlx::query_as(
            "SELECT board_id, min(lease_expires_at)-(extract(epoch FROM clock_timestamp())*1000)::bigint
             FROM board_tickets WHERE tenant_id=$1 AND holder IS NOT NULL GROUP BY board_id",
        )
        .bind(tenant)
        .fetch_all(&mut *connection)
        .await?)
    })
}
