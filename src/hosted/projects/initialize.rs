use super::*;

impl HostedBoard {
    pub(in crate::hosted) async fn initialize_in(
        &self,
        connection: &mut AnyConnection,
        limit: Option<usize>,
    ) -> DbResult<bool> {
        self.project_namespace(connection).await?;
        let existing: Option<Option<i64>> = sqlx::query_scalar(
            "SELECT retired_at FROM board_projects WHERE tenant_id=$1 AND id=$2",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .fetch_optional(&mut *connection)
        .await?;
        if existing.is_some_and(|retired| retired.is_some()) {
            return Err(fail("not_found", "a retired board identity cannot be reopened").into());
        }
        if existing.is_some() {
            return Ok(false);
        }
        if let (Some(limit), Some(owner)) = (limit, &self.owner) {
            let count: i64=sqlx::query_scalar(
                "SELECT count(*) FROM board_projects WHERE tenant_id=$1 AND owner_id=$2 AND retired_at IS NULL",
            ).bind(&self.tenant).bind(owner).fetch_one(&mut *connection).await?;
            if count >= i64::try_from(limit).unwrap_or(i64::MAX) {
                return Err(fail(
                    "plan_limit",
                    format!("this account or org already holds {count} project(s); limit {limit}"),
                )
                .into());
            }
        }
        let now = Self::server_now(connection).await?;
        sqlx::query(
            "INSERT INTO board_projects (tenant_id,id,source_hash,imported_at,owner_id,project_name,display_name)
             VALUES ($1,$2,'native',$3,$4,$5,$5)",
        ).bind(&self.tenant).bind(&self.project).bind(now).bind(&self.owner).bind(&self.name)
            .execute(&mut *connection).await?;
        for (key, value) in [
            ("schema_version", crate::SCHEMA_VERSION.to_string()),
            ("project", self.name.clone()),
            ("prefix", default_prefix(&self.name)),
            ("created_at", now.to_string()),
        ] {
            sqlx::query(
                "INSERT INTO board_metadata (tenant_id,board_id,key,value) VALUES ($1,$2,$3,$4)",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(key)
            .bind(value)
            .execute(&mut *connection)
            .await?;
        }
        for (key, value) in [
            (
                "settings",
                serde_json::to_string(&Settings::default()).expect("settings json"),
            ),
            (
                "workflow",
                serde_json::to_string(&Workflow::default_workflow()).expect("workflow json"),
            ),
        ] {
            sqlx::query(
                "INSERT INTO board_settings (tenant_id,board_id,key,value) VALUES ($1,$2,$3,$4)",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(key)
            .bind(value)
            .execute(&mut *connection)
            .await?;
        }
        sqlx::query("INSERT INTO board_fence (tenant_id,board_id,id,value) VALUES ($1,$2,1,0)")
            .bind(&self.tenant)
            .bind(&self.project)
            .execute(&mut *connection)
            .await?;
        Ok(true)
    }
}
