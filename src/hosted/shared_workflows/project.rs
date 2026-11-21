//! Project editing composes workflow selection into its existing transaction.
use super::*;

impl HostedBoard {
    pub(in crate::hosted) async fn select_project_workflow_in(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        owner: &str,
        selection: Option<(&str, i64)>,
        now: i64,
    ) -> DbResult<()> {
        if let Some((id, version)) = selection {
            let entry = self.entry(connection, owner, id).await?;
            expected(entry.version, version)?;
            if self
                .workflow_binding_in(connection)
                .await?
                .is_none_or(|binding| binding.id != id)
            {
                self.validate_replacement_in(connection, &entry.workflow)
                    .await?;
                self.bind_in(connection, owner, id, actor, now).await?;
            }
        } else if let Some(binding) = self.workflow_binding_in(connection).await? {
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            sqlx::query("DELETE FROM board_workflow_bindings WHERE tenant_id=$1 AND board_id=$2")
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
        Ok(())
    }
}
