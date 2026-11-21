use super::*;

impl HostedBoard {
    /// Edit one definition and every using board as one transaction. The host authorizes
    /// its entire owner namespace before calling this, not merely the originating board.
    pub async fn edit_shared_workflow(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        workflow: &Workflow,
    ) -> BridgeResult<SharedWorkflow> {
        human(actor)?;
        if workflow.id != id {
            return Err(invalid("a shared workflow id cannot change"));
        }
        workflow.validate().map_err(invalid)?;
        transaction!(&self.store, |connection| {
            let (owner, now) = self.catalog_scope(connection).await?;
            self.authorize_configuration(connection, actor, "admin.workflows.put")
                .await?;
            let receipt: Receipt<SharedWorkflow> = self
                .begin_mutation(connection, actor, "edit_shared_workflow")
                .await?;
            if let Receipt::Replay(saved) = receipt {
                let mut names = saved.used_by.clone();
                names.extend(
                    self.uses(connection, &owner, id)
                        .await?
                        .into_iter()
                        .map(|(_, name)| name),
                );
                names.sort();
                names.dedup();
                for name in names {
                    let mut board = self.clone();
                    board.name = name;
                    board
                        .authorize_configuration(connection, actor, "admin.workflows.put")
                        .await?;
                }
                return Ok(saved);
            }
            let entry = self.entry(connection, &owner, id).await?;
            expected(entry.version, version)?;
            if &entry.workflow == workflow {
                let result = self.projection(connection, &owner, entry).await?;
                return receipt.complete(connection, result).await;
            }
            let uses = self.uses(connection, &owner, id).await?;
            let mut boards = Vec::new();
            for (id, name) in &uses {
                let mut board = Self::new(self.store.clone(), &self.tenant, id)?;
                board.name = name.clone();
                board.scope(connection).await?;
                board
                    .authorize_configuration(connection, actor, "admin.workflows.put")
                    .await?;
                board.validate_replacement_in(connection, workflow).await?;
                boards.push(board);
            }
            sqlx::query("UPDATE board_workflows SET value=$4,version=version+1 WHERE tenant_id=$1 AND owner_id=$2 AND id=$3")
                .bind(&self.tenant).bind(&owner).bind(id)
                .bind(serde_json::to_string(workflow).expect("workflow json"))
                .execute(&mut *connection).await?;
            for board in boards {
                board.advance_workflow(connection).await?;
                board
                    .event(
                        connection,
                        actor.id,
                        now,
                        actions::BOARD_WORKFLOW_REPLACED,
                        None,
                        None,
                        serde_json::json!({"id":id,"shared_version":version+1}),
                    )
                    .await?;
            }
            self.catalog_event(connection, &entry.source, actor, now, actions::WORKFLOW_EDITED,
                serde_json::json!({"id":id,"version":version+1,"boards":uses.into_iter().map(|(_,name)| name).collect::<Vec<_>>()})).await?;
            let entry = self.entry(connection, &owner, id).await?;
            let result = self.projection(connection, &owner, entry).await?;
            receipt.complete(connection, result).await
        })
    }

    /// Delete an unused definition. Its history remains on the publishing board's chain.
    pub async fn delete_shared_workflow(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
    ) -> BridgeResult<()> {
        human(actor)?;
        transaction!(&self.store, |connection| {
            let (owner, now) = self.catalog_scope(connection).await?;
            self.authorize_configuration(connection, actor, "admin.workflows.delete")
                .await?;
            let receipt = self
                .begin_mutation(connection, actor, "delete_shared_workflow")
                .await?;
            if let Receipt::Replay(saved) = receipt {
                return Ok(saved);
            }
            let entry = self.entry(connection, &owner, id).await?;
            expected(entry.version, version)?;
            if !self.uses(connection, &owner, id).await?.is_empty() {
                return Err(fail(
                    "in_use",
                    "detach the boards using this shared workflow before deleting it",
                )
                .into());
            }
            sqlx::query("DELETE FROM board_workflows WHERE tenant_id=$1 AND owner_id=$2 AND id=$3")
                .bind(&self.tenant)
                .bind(&owner)
                .bind(id)
                .execute(&mut *connection)
                .await?;
            self.catalog_event(
                connection,
                &entry.source,
                actor,
                now,
                actions::WORKFLOW_DELETED,
                serde_json::json!({"id":id,"version":version}),
            )
            .await?;
            receipt.complete(connection, ()).await
        })
    }
}
