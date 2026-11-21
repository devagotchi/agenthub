//! One definition shared by boards in one owner namespace. Catalog changes take the
//! namespace lock before board locks; ordinary board operations only take a board lock.
//!
//! Owns: `board_workflows` and `board_workflow_bindings`.
//! Serves: the hosted/synchronous catalog API; the host supplies authorized UI routes.
//! Settings: workflow definitions and bindings, managed through the host's board pages.
//! Depends on: the existing board configuration, chain/outbox and PostgreSQL store.
use super::*;
use crate::{SharedWorkflow, WorkflowBinding, actions};

mod bindings;
mod mutations;
mod project;

struct Entry {
    workflow: Workflow,
    version: i64,
    source: String,
}

fn human(actor: &Actor<'_>) -> BridgeResult<()> {
    if actor.human {
        Ok(())
    } else {
        Err(fail(
            "humans_only",
            "only a person manages shared workflows",
        ))
    }
}

fn expected(actual: i64, expected: i64) -> BridgeResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(fail(
            "version_conflict",
            "shared workflow changed; reload before saving",
        ))
    }
}

impl HostedBoard {
    pub async fn workflow_configuration(&self) -> BridgeResult<crate::WorkflowConfiguration> {
        transaction!(&self.store, |connection| {
            self.scope(connection).await?;
            Ok(crate::WorkflowConfiguration {
                configuration: self.configuration_state_in(connection).await?,
                shared_workflow: self.workflow_binding_in(connection).await?,
            })
        })
    }

    async fn catalog_scope(&self, connection: &mut AnyConnection) -> DbResult<(String, i64)> {
        self.store.scope(connection, &self.tenant).await?;
        let owner: Option<Option<String>> =
            sqlx::query_scalar("SELECT owner_id FROM board_projects WHERE tenant_id=$1 AND id=$2")
                .bind(&self.tenant)
                .bind(&self.project)
                .fetch_optional(&mut *connection)
                .await?;
        let owner = owner
            .ok_or_else(|| fail("not_found", "board does not exist"))?
            .ok_or_else(|| {
                fail(
                    "not_available",
                    "shared workflows require an owner namespace",
                )
            })?;
        self.catalog_namespace(connection, &owner).await?;
        let now = self.scope(connection).await?;
        Ok((owner, now))
    }

    pub(in crate::hosted) async fn catalog_namespace(
        &self,
        connection: &mut AnyConnection,
        owner: &str,
    ) -> DbResult<()> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(format!("board_workflows:{}:{owner}", self.tenant))
            .execute(&mut *connection)
            .await?;
        Ok(())
    }

    async fn entry(
        &self,
        connection: &mut AnyConnection,
        owner: &str,
        id: &str,
    ) -> DbResult<Entry> {
        let row: Option<(String, i64, String)> = sqlx::query_as(
            "SELECT value,version,source_board FROM board_workflows
             WHERE tenant_id=$1 AND owner_id=$2 AND id=$3",
        )
        .bind(&self.tenant)
        .bind(owner)
        .bind(id)
        .fetch_optional(&mut *connection)
        .await?;
        let (value, version, source) =
            row.ok_or_else(|| fail("not_found", "shared workflow does not exist"))?;
        Ok(Entry {
            workflow: decode(&value)?,
            version,
            source,
        })
    }

    async fn uses(
        &self,
        connection: &mut AnyConnection,
        owner: &str,
        id: &str,
    ) -> DbResult<Vec<(String, String)>> {
        Ok(sqlx::query_as(
            "SELECT p.id,p.project_name FROM board_workflow_bindings b
             JOIN board_projects p ON p.tenant_id=b.tenant_id AND p.id=b.board_id AND p.owner_id=b.owner_id
             WHERE b.tenant_id=$1 AND b.owner_id=$2 AND b.workflow_id=$3 ORDER BY p.id",
        ).bind(&self.tenant).bind(owner).bind(id).fetch_all(&mut *connection).await?)
    }

    async fn projection(
        &self,
        connection: &mut AnyConnection,
        owner: &str,
        entry: Entry,
    ) -> DbResult<SharedWorkflow> {
        let mut used_by: Vec<String> = self
            .uses(connection, owner, &entry.workflow.id)
            .await?
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        used_by.sort();
        Ok(SharedWorkflow {
            workflow: entry.workflow,
            version: entry.version,
            used_by,
        })
    }

    /// The host must authorize the entire owner namespace, not just one project's grant.
    pub async fn shared_workflows(&self) -> BridgeResult<Vec<SharedWorkflow>> {
        transaction!(&self.store, |connection| {
            let (owner, _) = self.catalog_scope(connection).await?;
            let ids: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM board_workflows WHERE tenant_id=$1 AND owner_id=$2 ORDER BY id",
            )
            .bind(&self.tenant)
            .bind(&owner)
            .fetch_all(&mut *connection)
            .await?;
            let mut out = Vec::new();
            for id in ids {
                let entry = self.entry(connection, &owner, &id).await?;
                out.push(self.projection(connection, &owner, entry).await?);
            }
            Ok(out)
        })
    }

    pub(in crate::hosted) async fn workflow_binding_in(
        &self,
        connection: &mut AnyConnection,
    ) -> DbResult<Option<WorkflowBinding>> {
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT w.id,w.version FROM board_workflow_bindings b JOIN board_workflows w
             ON w.tenant_id=b.tenant_id AND w.owner_id=b.owner_id AND w.id=b.workflow_id
             WHERE b.tenant_id=$1 AND b.board_id=$2",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .fetch_optional(&mut *connection)
        .await?;
        Ok(row.map(|(id, version)| WorkflowBinding { id, version }))
    }

    pub(in crate::hosted) async fn shared_workflow_value(
        &self,
        connection: &mut AnyConnection,
    ) -> DbResult<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT w.value FROM board_workflow_bindings b JOIN board_workflows w
             ON w.tenant_id=b.tenant_id AND w.owner_id=b.owner_id AND w.id=b.workflow_id
             WHERE b.tenant_id=$1 AND b.board_id=$2",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .fetch_optional(&mut *connection)
        .await?)
    }

    async fn advance_workflow(&self, connection: &mut AnyConnection) -> DbResult<()> {
        sqlx::query("UPDATE board_settings SET version=version+1 WHERE tenant_id=$1 AND board_id=$2 AND key='workflow'")
            .bind(&self.tenant).bind(&self.project).execute(&mut *connection).await?;
        sqlx::query("UPDATE board_projects SET version=version+1 WHERE tenant_id=$1 AND id=$2")
            .bind(&self.tenant)
            .bind(&self.project)
            .execute(&mut *connection)
            .await?;
        Ok(())
    }

    async fn catalog_event(
        &self,
        connection: &mut AnyConnection,
        source: &str,
        actor: &Actor<'_>,
        now: i64,
        action: &str,
        data: serde_json::Value,
    ) -> DbResult<()> {
        let anchor = Self::new(self.store.clone(), &self.tenant, source)?;
        anchor.scope(connection).await?;
        anchor
            .event(connection, actor.id, now, action, None, None, data)
            .await
    }
}
