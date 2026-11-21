//! Owned project lifecycle. Retired identities and their append-only history remain.
use super::*;

mod history;
mod initialize;
mod list;
mod mutations;
mod resolve;
mod types;
pub use types::{ProjectEdit, ProjectRecord};

impl HostedBoard {
    async fn project_record_in(&self, connection: &mut AnyConnection) -> DbResult<ProjectRecord> {
        let row=sqlx::query(
            "SELECT project_name,display_name,version,retired_at,
             (SELECT workflow_id FROM board_workflow_bindings WHERE tenant_id=$1 AND board_id=$2) AS workflow_id
             FROM board_projects WHERE tenant_id=$1 AND id=$2",
        ).bind(&self.tenant).bind(&self.project).fetch_one(&mut *connection).await?;
        Ok(ProjectRecord {
            board_id: self.project.clone(),
            id: row.try_get("project_name")?,
            name: row.try_get("display_name")?,
            version: row.try_get("version")?,
            retired_at: row.try_get("retired_at")?,
            workflow_id: row.try_get("workflow_id")?,
        })
    }

    async fn project_owner(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
    ) -> DbResult<()> {
        if !actor.human || self.owner.as_deref() != Some(actor.id) {
            return Err(fail(
                "not_owner",
                "only this account's person manages its project lifecycle",
            )
            .into());
        }
        let directory = self
            .principals
            .as_ref()
            .ok_or_else(|| fail("not_granted", "project identity directory is unavailable"))?;
        directory
            .authorize_request(connection, &self.tenant, actor.id, actor)
            .await
    }

    pub async fn project_record(&self) -> BridgeResult<ProjectRecord> {
        transaction!(&self.store, |connection| {
            self.scope(connection).await?;
            self.project_record_in(connection).await
        })
    }

    pub(in crate::hosted) async fn project_namespace(
        &self,
        connection: &mut AnyConnection,
    ) -> DbResult<()> {
        self.store.scope(connection, &self.tenant).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('board_projects:' || $1))")
            .bind(&self.tenant)
            .execute(&mut *connection)
            .await?;
        Ok(())
    }
}
