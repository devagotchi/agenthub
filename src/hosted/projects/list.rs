use super::*;

impl HostedBoard {
    /// The authenticated account's active or retired projects, with one lookahead row.
    pub async fn owned_projects(
        store: &Store,
        tenant: &str,
        owner: &str,
        retired: bool,
        after: Option<&str>,
    ) -> BridgeResult<Vec<ProjectRecord>> {
        let (name, id) = match after {
            None => ("", ""),
            Some(cursor) => cursor
                .split_once('/')
                .filter(|(name, id)| crate::valid_name(name) && id.len() <= 66)
                .ok_or_else(|| invalid("invalid project cursor"))?,
        };
        transaction!(store, |connection| {
            store.scope(connection, tenant).await?;
            let rows=sqlx::query(
                "SELECT p.id,p.project_name,p.display_name,p.version,p.retired_at,b.workflow_id
                 FROM board_projects p LEFT JOIN board_workflow_bindings b ON b.tenant_id=p.tenant_id AND b.board_id=p.id
                 WHERE p.tenant_id=$1 AND p.owner_id=$2 AND (p.retired_at IS NOT NULL)=$3
                 AND (p.project_name COLLATE \"C\",p.id COLLATE \"C\")>($4,$5)
                 ORDER BY p.project_name COLLATE \"C\",p.id COLLATE \"C\" LIMIT 101",
            ).bind(tenant).bind(owner).bind(retired).bind(name).bind(id).fetch_all(&mut *connection).await?;
            rows.iter()
                .map(|row| {
                    Ok(ProjectRecord {
                        board_id: row.try_get("id")?,
                        id: row.try_get("project_name")?,
                        name: row.try_get("display_name")?,
                        version: row.try_get("version")?,
                        retired_at: row.try_get("retired_at")?,
                        workflow_id: row.try_get("workflow_id")?,
                    })
                })
                .collect()
        })
    }
}
