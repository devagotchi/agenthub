//! What a host needs to know about the boards in a tenant without opening one: the names an
//! owner holds, each board with its owner, whether a board exists, and when each board's
//! first lease lapses. The hub's board registry and its signal sink ask these instead of
//! running their own SQL on `board_*` tables, so a change to the tables changes them here.

use devagotchi_contracts::BridgeResult;
use devagotchi_store::{Store, transaction};

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
            "SELECT project_name FROM board_projects WHERE tenant_id=$1 AND owner_id=$2
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
            "SELECT id, owner_id, project_name FROM board_projects WHERE tenant_id=$1 ORDER BY id",
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
            "SELECT count(*) FROM board_projects WHERE tenant_id=$1 AND owner_id=$2 AND project_name=$3",
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
