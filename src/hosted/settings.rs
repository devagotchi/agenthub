//! A board's settings and workflow: read, changed field by field (`set_project`), replaced
//! whole (`put_settings`, `put_workflow`). A change that changes nothing writes nothing and
//! announces nothing (rule 6).

use super::*;
use crate::ProjectSettings;
use crate::chain::actions;

impl HostedBoard {
    pub async fn settings(&self) -> BridgeResult<Settings> {
        transaction!(&self.store, |connection| {
            self.scope(connection).await?;
            self.configuration(connection, "settings").await
        })
    }

    pub async fn workflow(&self) -> BridgeResult<Workflow> {
        transaction!(&self.store, |connection| {
            self.scope(connection).await?;
            self.configuration(connection, "workflow").await
        })
    }

    /// Replace the settings whole. Settings equal to the board's write nothing; otherwise
    /// `board.settings_replaced` names the top-level keys that changed.
    pub async fn put_settings(&self, actor: &Actor<'_>, settings: &Settings) -> BridgeResult<()> {
        if settings.lease_min_ttl_ms <= 0
            || settings.lease_max_ttl_ms < settings.lease_min_ttl_ms
            || settings.lease_ttl_ms < settings.lease_min_ttl_ms
            || settings.lease_ttl_ms > settings.lease_max_ttl_ms
        {
            return Err(invalid(
                "lease ttl bounds must satisfy min <= default <= max, min > 0",
            ));
        }
        let new = serde_json::to_value(settings).expect("settings json");
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let current: Settings = self.configuration(connection, "settings").await?;
            let current = serde_json::to_value(&current).expect("settings json");
            let changed = changed_keys(&current, &new);
            if changed.is_empty() {
                return Ok(());
            }
            self.save_configuration(connection, "settings", settings)
                .await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::BOARD_SETTINGS_REPLACED,
                None,
                None,
                serde_json::json!({ "changed": changed }),
            )
            .await?;
            Ok(())
        })
    }

    pub(super) async fn save_configuration<T: serde::Serialize>(
        &self,
        connection: &mut AnyConnection,
        key: &str,
        value: &T,
    ) -> DbResult<()> {
        let value = serde_json::to_string(value).map_err(|error| invalid(error.to_string()))?;
        sqlx::query(
            "UPDATE board_settings SET value=$4 WHERE tenant_id=$1 AND board_id=$2 AND key=$3",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(key)
        .bind(value)
        .execute(&mut *connection)
        .await?;
        Ok(())
    }

    /// Name the project's repo and environments: each field given that differs from the
    /// board's replaces it, and `board.settings_changed` names what changed. Nothing given,
    /// or only what the board already has, writes nothing.
    pub async fn set_project(
        &self,
        actor: &Actor<'_>,
        patch: &ProjectSettings,
    ) -> BridgeResult<Settings> {
        let repo = patch.repo.as_deref().map(crate::check_repo).transpose()?;
        let environments = patch
            .environments
            .as_deref()
            .map(crate::check_environments)
            .transpose()?;
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let mut settings: Settings = self.configuration(connection, "settings").await?;
            let mut changed = serde_json::Map::new();
            if let Some(repo) = &repo
                && settings.repo.as_ref() != Some(repo)
            {
                settings.repo = Some(repo.clone());
                changed.insert("repo".into(), serde_json::json!(repo));
            }
            if let Some(environments) = &environments
                && &settings.environments != environments
            {
                settings.environments = environments.clone();
                changed.insert("environments".into(), serde_json::json!(environments));
            }
            if !changed.is_empty() {
                self.save_configuration(connection, "settings", &settings)
                    .await?;
                self.event(
                    connection,
                    actor.id,
                    now,
                    actions::BOARD_SETTINGS_CHANGED,
                    None,
                    None,
                    changed.into(),
                )
                .await?;
            }
            Ok(settings)
        })
    }

    /// Replace the workflow. Every existing ticket's state must still exist in it; the same
    /// workflow again writes nothing.
    pub async fn put_workflow(&self, actor: &Actor<'_>, workflow: &Workflow) -> BridgeResult<()> {
        workflow.validate().map_err(invalid)?;
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let current: Workflow = self.configuration(connection, "workflow").await?;
            if &current == workflow {
                return Ok(());
            }
            let states: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT state FROM board_tickets WHERE tenant_id=$1 AND board_id=$2",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .fetch_all(&mut *connection)
            .await?;
            if let Some(unknown) = states.iter().find(|state| workflow.state(state).is_none()) {
                return Err(invalid(format!(
                    "tickets are in state {unknown:?}, which the new workflow lacks"
                ))
                .into());
            }
            self.save_configuration(connection, "workflow", workflow)
                .await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::BOARD_WORKFLOW_REPLACED,
                None,
                None,
                serde_json::json!({"id":workflow.id}),
            )
            .await?;
            Ok(())
        })
    }
}

/// The top-level keys whose values differ between two settings objects, in key order.
fn changed_keys(current: &serde_json::Value, new: &serde_json::Value) -> Vec<String> {
    let empty = serde_json::Map::new();
    let current = current.as_object().unwrap_or(&empty);
    let new = new.as_object().unwrap_or(&empty);
    let mut keys: Vec<String> = current
        .keys()
        .chain(new.keys())
        .filter(|key| current.get(*key) != new.get(*key))
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();
    keys
}
