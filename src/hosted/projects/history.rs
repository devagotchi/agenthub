use super::*;

impl HostedBoard {
    /// An explicit archived generation, never a logical name that might now be reused.
    pub async fn archived_generation(
        store: Store,
        tenant: &str,
        owner: &str,
        id: &str,
    ) -> BridgeResult<Option<Self>> {
        let mut board = Self::new(store.clone(), tenant, id)?;
        let name: Option<String> = transaction!(&store, |connection| {
            store.scope(connection, tenant).await?;
            Ok(sqlx::query_scalar("SELECT project_name FROM board_projects WHERE tenant_id=$1 AND owner_id=$2 AND id=$3 AND retired_at IS NOT NULL")
                .bind(tenant).bind(owner).bind(id).fetch_optional(&mut *connection).await?)
        })?;
        Ok(name.map(|name| {
            board.name = name;
            board.owner = Some(owner.into());
            board
        }))
    }

    pub async fn archived_project_events(
        &self,
        actor: &Actor<'_>,
        after: i64,
        limit: usize,
    ) -> BridgeResult<(ProjectRecord, Vec<Event>)> {
        if after < 0 || !(1..=100).contains(&limit) {
            return Err(invalid("invalid archive page bounds"));
        }
        transaction!(&self.store, |connection| {
            self.store.scope(connection, &self.tenant).await?;
            self.lock_project_any(connection).await?;
            self.project_owner(connection, actor).await?;
            let project = self.project_record_in(connection).await?;
            if project.retired_at.is_none() {
                return Err(fail("not_found", "archived project does not exist").into());
            }
            let rows: Vec<String>=sqlx::query_scalar(
                "SELECT row_to_json(e)::text FROM board_events e WHERE tenant_id=$1 AND board_id=$2 AND seq>$3 ORDER BY seq LIMIT $4",
            ).bind(&self.tenant).bind(&self.project).bind(after).bind(limit as i64)
                .fetch_all(&mut *connection).await?;
            Ok((project, super::super::events::decode_events(rows)?))
        })
    }
}
