use super::*;

impl HostedBoard {
    /// Resolve the current generation by owner/name, serialized with creation/removal.
    /// New identities are allocated outside the retryable transaction.
    pub async fn resolve_owner(
        store: Store,
        tenant: &str,
        owner: &str,
        project: &str,
        create: bool,
        limit: Option<usize>,
    ) -> BridgeResult<Option<(Self, bool)>> {
        let original = Self::for_owner(store.clone(), tenant, owner, project)?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|error| fail("board_random", error.to_string()))?;
        let fresh_id = format!("b-{}", hex::encode(random));
        transaction!(&store, |connection| {
            original.project_namespace(connection).await?;
            let current: Option<String>=sqlx::query_scalar(
                "SELECT id FROM board_projects WHERE tenant_id=$1 AND owner_id=$2 AND project_name=$3 AND retired_at IS NULL",
            ).bind(tenant).bind(owner).bind(project).fetch_optional(&mut *connection).await?;
            if let Some(id) = current {
                let mut board = original.clone();
                board.project = id;
                return Ok(Some((board, false)));
            }
            if !create {
                return Ok(None);
            }
            let historical: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM board_projects WHERE tenant_id=$1 AND id=$2",
            )
            .bind(tenant)
            .bind(&original.project)
            .fetch_one(&mut *connection)
            .await?;
            let mut board = original.clone();
            if historical > 0 {
                board.project = fresh_id.clone();
            }
            let created = board.initialize_in(connection, limit).await?;
            Ok(Some((board, created)))
        })
    }

    /// Only a host's human owner administration may use this for retirement replay.
    /// It does not reopen a retired board for ordinary reads or mutations.
    pub async fn retirement_target(
        store: Store,
        tenant: &str,
        owner: &str,
        project: &str,
    ) -> BridgeResult<Option<Self>> {
        let original = Self::for_owner(store.clone(), tenant, owner, project)?;
        transaction!(&store, |connection| {
            original.project_namespace(connection).await?;
            let id: Option<String>=sqlx::query_scalar(
                "SELECT id FROM board_projects WHERE tenant_id=$1 AND owner_id=$2 AND project_name=$3
                 ORDER BY retired_at DESC NULLS FIRST,imported_at DESC,id LIMIT 1",
            ).bind(tenant).bind(owner).bind(project).fetch_optional(&mut *connection).await?;
            Ok(id.map(|id| {
                let mut board = original.clone();
                board.project = id;
                board
            }))
        })
    }
}
