use super::*;
use serde_json::json;

impl HostedBoard {
    /// Create/edit the logical account project in one transaction, including its
    /// workflow binding, defaults, metadata, audit and permanent request identity.
    pub async fn put_owned_project(
        &self,
        actor: &Actor<'_>,
        edit: &ProjectEdit,
        key: Option<&str>,
        limit: Option<usize>,
    ) -> BridgeResult<(ProjectRecord, bool)> {
        edit.check()?;
        if edit.id != self.name {
            return Err(invalid("project id disagrees with its namespace"));
        }
        let owner = self
            .owner
            .as_deref()
            .ok_or_else(|| fail("not_owner", "project requires an account owner"))?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|error| fail("board_random", error.to_string()))?;
        let fresh_id = format!("b-{}", hex::encode(random));
        transaction!(&self.store, |connection| {
            self.project_namespace(connection).await?;
            self.catalog_namespace(connection, owner).await?;
            self.project_owner(connection, actor).await?;
            let previous: Option<(String,Option<i64>)>=sqlx::query_as(
                "SELECT id,retired_at FROM board_projects WHERE tenant_id=$1 AND owner_id=$2 AND project_name=$3
                 ORDER BY retired_at DESC NULLS FIRST,imported_at DESC,id LIMIT 1",
            ).bind(&self.tenant).bind(owner).bind(&self.name).fetch_optional(&mut *connection).await?;
            let active = previous
                .as_ref()
                .is_some_and(|(_, retired)| retired.is_none());
            let mut board = self.clone();
            if let Some((id, retired)) = &previous {
                board.project = if retired.is_none() || edit.version > 0 {
                    id.clone()
                } else {
                    fresh_id.clone()
                };
            }
            let fresh = !active && edit.version == 0;
            if fresh {
                board.initialize_in(connection, limit).await?;
            } else {
                board.lock_project_any(connection).await?;
            }
            board
                .authorize_configuration(connection, actor, "admin.projects.put")
                .await?;
            let board = match key {
                Some(key) => board.requested(actor.id, key, "put_project", edit)?,
                None => board,
            };
            let receipt = board
                .begin_mutation(connection, actor, "put_project")
                .await?;
            if let Receipt::Replay(saved) = receipt {
                return Ok((saved, true));
            }
            let current = board.project_record_in(connection).await?;
            if current.retired_at.is_some() {
                return Err(fail("not_found", "project is retired").into());
            }
            if edit.version == 0 && active {
                return Err(fail("already_exists", "project already exists").into());
            }
            if !fresh && current.version != edit.version {
                return Err(
                    fail("version_conflict", "project changed; reload before saving").into(),
                );
            }
            let now = Self::server_now(connection).await?;
            board
                .select_project_workflow_in(
                    connection,
                    actor,
                    owner,
                    edit.workflow_id.as_deref().zip(edit.workflow_version),
                    now,
                )
                .await?;
            if !fresh && current.name == edit.name.trim() && current.workflow_id == edit.workflow_id
            {
                return receipt
                    .complete(connection, current)
                    .await
                    .map(|saved| (saved, false));
            }
            // Workflow changes can advance the project version internally; this single
            // intent publishes exactly the next metadata version, or version 1 on create.
            sqlx::query(
                "UPDATE board_projects SET display_name=$3,version=$4 WHERE tenant_id=$1 AND id=$2",
            )
            .bind(&board.tenant)
            .bind(&board.project)
            .bind(edit.name.trim())
            .bind(if fresh { 1 } else { edit.version + 1 })
            .execute(&mut *connection)
            .await?;
            board.event(connection,actor.id,now,crate::actions::PROJECT_SAVED,None,None,
                json!({"project":edit.id,"name":edit.name.trim(),"created":fresh,"workflow_id":edit.workflow_id})).await?;
            let result = board.project_record_in(connection).await?;
            receipt
                .complete(connection, result)
                .await
                .map(|saved| (saved, false))
        })
    }

    pub async fn retire_owned_project(
        &self,
        actor: &Actor<'_>,
        version: i64,
        key: Option<&str>,
    ) -> BridgeResult<(ProjectRecord, bool)> {
        if version <= 0 {
            return Err(invalid("project removal requires its positive version"));
        }
        let owner = self
            .owner
            .as_deref()
            .ok_or_else(|| fail("not_owner", "project requires an account owner"))?;
        let requested = key
            .map(|key| {
                self.requested(
                    actor.id,
                    key,
                    "retire_project",
                    &json!({"project":self.name,"version":version}),
                )
            })
            .transpose()?;
        let board = requested.as_ref().unwrap_or(self);
        transaction!(&self.store, |connection| {
            board.project_namespace(connection).await?;
            board.catalog_namespace(connection, owner).await?;
            board.lock_project_any(connection).await?;
            board.project_owner(connection, actor).await?;
            board
                .authorize_configuration(connection, actor, "admin.projects.delete")
                .await?;
            let receipt = board
                .begin_mutation(connection, actor, "retire_project")
                .await?;
            if let Receipt::Replay(saved) = receipt {
                return Ok((saved, true));
            }
            let current = board.project_record_in(connection).await?;
            if current.retired_at.is_some() {
                return Err(fail("not_found", "project is retired").into());
            }
            if current.version != version {
                return Err(fail(
                    "version_conflict",
                    "project changed; reload before removing",
                )
                .into());
            }
            let tasks: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM board_tickets WHERE tenant_id=$1 AND board_id=$2",
            )
            .bind(&board.tenant)
            .bind(&board.project)
            .fetch_one(&mut *connection)
            .await?;
            let definitions: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM board_workflows WHERE tenant_id=$1 AND source_board=$2",
            )
            .bind(&board.tenant)
            .bind(&board.project)
            .fetch_one(&mut *connection)
            .await?;
            if tasks > 0 || definitions > 0 {
                return Err(fail(
                    "in_use",
                    "project still contains tasks or anchors a shared workflow's history",
                )
                .into());
            }
            board
                .principals
                .as_ref()
                .expect("checked project directory")
                .project_retirement(connection, &board.tenant, owner, &board.name)
                .await?;
            sqlx::query("DELETE FROM board_workflow_bindings WHERE tenant_id=$1 AND board_id=$2")
                .bind(&board.tenant)
                .bind(&board.project)
                .execute(&mut *connection)
                .await?;
            let now = Self::server_now(connection).await?;
            board
                .event(
                    connection,
                    actor.id,
                    now,
                    crate::actions::PROJECT_RETIRED,
                    None,
                    None,
                    json!({"project":board.name,"version":version}),
                )
                .await?;
            sqlx::query("UPDATE board_projects SET retired_at=$3,version=version+1 WHERE tenant_id=$1 AND id=$2")
                .bind(&board.tenant).bind(&board.project).bind(now).execute(&mut *connection).await?;
            let result = board.project_record_in(connection).await?;
            receipt
                .complete(connection, result)
                .await
                .map(|saved| (saved, false))
        })
    }

    pub(in crate::hosted) async fn lock_project_any(
        &self,
        connection: &mut AnyConnection,
    ) -> DbResult<()> {
        let found: Option<String> = sqlx::query_scalar(
            "SELECT id FROM board_projects WHERE tenant_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .fetch_optional(&mut *connection)
        .await?;
        if found.is_none() {
            return Err(fail("not_found", "project does not exist").into());
        }
        Ok(())
    }
}
