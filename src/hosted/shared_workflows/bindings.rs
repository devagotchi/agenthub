use super::*;

impl HostedBoard {
    /// Publish this board's current workflow and bind it, atomically. The immutable id
    /// names the definition within this board owner's catalog.
    pub async fn publish_workflow(
        &self,
        actor: &Actor<'_>,
        board_version: i64,
        id: &str,
        name: &str,
    ) -> BridgeResult<SharedWorkflow> {
        human(actor)?;
        if !crate::valid_name(id) {
            return Err(invalid("shared workflow id must be a valid name"));
        }
        transaction!(&self.store, |connection| {
            let (owner, now) = self.catalog_scope(connection).await?;
            self.authorize_configuration(connection, actor, "admin.workflows.put")
                .await?;
            self.expected_configuration(connection, "workflow", board_version)
                .await?;
            if self.workflow_binding_in(connection).await?.is_some() {
                return Err(fail(
                    "shared_workflow",
                    "detach this board before publishing another definition",
                )
                .into());
            }
            let mut workflow: Workflow = self.configuration(connection, "workflow").await?;
            workflow.id = id.into();
            workflow.name = name.trim().into();
            workflow.validate().map_err(invalid)?;
            let inserted = sqlx::query(
                "INSERT INTO board_workflows (tenant_id,owner_id,id,source_board,value)
                 VALUES ($1,$2,$3,$4,$5) ON CONFLICT (tenant_id,owner_id,id) DO NOTHING",
            )
            .bind(&self.tenant)
            .bind(&owner)
            .bind(id)
            .bind(&self.project)
            .bind(serde_json::to_string(&workflow).expect("workflow json"))
            .execute(&mut *connection)
            .await?
            .rows_affected();
            if inserted != 1 {
                return Err(fail("already_exists", "a shared workflow has that id").into());
            }
            self.bind_in(connection, &owner, id, actor, now).await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::WORKFLOW_PUBLISHED,
                None,
                None,
                serde_json::json!({"id":id,"version":1}),
            )
            .await?;
            let entry = self.entry(connection, &owner, id).await?;
            self.projection(connection, &owner, entry).await
        })
    }

    async fn bind_in(
        &self,
        connection: &mut AnyConnection,
        owner: &str,
        id: &str,
        actor: &Actor<'_>,
        now: i64,
    ) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO board_workflow_bindings (tenant_id,board_id,owner_id,workflow_id) VALUES ($1,$2,$3,$4)
             ON CONFLICT (tenant_id,board_id) DO UPDATE SET owner_id=EXCLUDED.owner_id,workflow_id=EXCLUDED.workflow_id",
        ).bind(&self.tenant).bind(&self.project).bind(owner).bind(id).execute(&mut *connection).await?;
        self.advance_workflow(connection).await?;
        self.event(
            connection,
            actor.id,
            now,
            actions::WORKFLOW_BOUND,
            None,
            None,
            serde_json::json!({"id":id}),
        )
        .await
    }

    /// Bind to exactly the definition the person selected. Both versions must still match.
    pub async fn bind_workflow(
        &self,
        actor: &Actor<'_>,
        board_version: i64,
        id: &str,
        catalog_version: i64,
    ) -> BridgeResult<crate::ConfigurationState> {
        human(actor)?;
        transaction!(&self.store, |connection| {
            let (owner, now) = self.catalog_scope(connection).await?;
            self.authorize_configuration(connection, actor, "admin.workflows.put")
                .await?;
            self.expected_configuration(connection, "workflow", board_version)
                .await?;
            let entry = self.entry(connection, &owner, id).await?;
            expected(entry.version, catalog_version)?;
            if self
                .workflow_binding_in(connection)
                .await?
                .is_none_or(|b| b.id != id)
            {
                self.validate_replacement_in(connection, &entry.workflow)
                    .await?;
                self.bind_in(connection, &owner, id, actor, now).await?;
            }
            self.configuration_state_in(connection).await
        })
    }

    /// Keep the current effective workflow as this board's independent definition.
    pub async fn detach_workflow(
        &self,
        actor: &Actor<'_>,
        board_version: i64,
    ) -> BridgeResult<crate::ConfigurationState> {
        human(actor)?;
        transaction!(&self.store, |connection| {
            let (_, now) = self.catalog_scope(connection).await?;
            self.authorize_configuration(connection, actor, "admin.workflows.put")
                .await?;
            self.expected_configuration(connection, "workflow", board_version)
                .await?;
            if let Some(binding) = self.workflow_binding_in(connection).await? {
                let workflow: Workflow = self.configuration(connection, "workflow").await?;
                sqlx::query(
                    "DELETE FROM board_workflow_bindings WHERE tenant_id=$1 AND board_id=$2",
                )
                .bind(&self.tenant)
                .bind(&self.project)
                .execute(&mut *connection)
                .await?;
                self.save_configuration(connection, "workflow", &workflow)
                    .await?;
                self.event(
                    connection,
                    actor.id,
                    now,
                    actions::WORKFLOW_DETACHED,
                    None,
                    None,
                    serde_json::json!({"id":binding.id,"version":binding.version}),
                )
                .await?;
            }
            self.configuration_state_in(connection).await
        })
    }
}
