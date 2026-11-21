//! A board's settings and workflow: read, changed field by field (`set_project`), replaced
//! whole (`put_settings`, `put_workflow`). A change that changes nothing writes nothing and
//! announces nothing (rule 6).

use super::*;
use crate::chain::actions;
use crate::{ConfigurationState, ProjectSettings, SettingsEdit};

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
        crate::settings::check_priority_labels(settings.priority_labels.as_deref())?;
        crate::settings::check_note_settings(
            settings.heartbeat_hint_ms,
            settings.note_kinds.as_deref(),
        )?;
        if settings.lease_min_ttl_ms <= 0
            || settings.lease_max_ttl_ms < settings.lease_min_ttl_ms
            || settings.lease_ttl_ms < settings.lease_min_ttl_ms
            || settings.lease_ttl_ms > settings.lease_max_ttl_ms
        {
            return Err(invalid(
                "lease ttl bounds must satisfy min <= default <= max, min > 0",
            ));
        }
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            self.replace_settings_in(connection, actor, now, settings)
                .await
        })
    }

    async fn replace_settings_in(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        now: i64,
        settings: &Settings,
    ) -> DbResult<()> {
        self.authorize_configuration(connection, actor, "admin.settings.put")
            .await?;
        let current: Settings = self.configuration(connection, "settings").await?;
        let changed = changed_keys(
            &serde_json::to_value(&current).expect("settings json"),
            &serde_json::to_value(settings).expect("settings json"),
        );
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
            serde_json::json!({"changed":changed}),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn save_configuration<T: serde::Serialize>(
        &self,
        connection: &mut AnyConnection,
        key: &str,
        value: &T,
    ) -> DbResult<()> {
        let value = serde_json::to_string(value).map_err(|error| invalid(error.to_string()))?;
        sqlx::query(
            "UPDATE board_settings SET value=$4,version=version+1 WHERE tenant_id=$1 AND board_id=$2 AND key=$3",
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
            self.authorize_configuration(connection, actor, "admin.projects.put")
                .await?;
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
            self.replace_workflow_in(connection, actor, now, workflow)
                .await
        })
    }

    async fn replace_workflow_in(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        now: i64,
        workflow: &Workflow,
    ) -> DbResult<()> {
        self.authorize_configuration(connection, actor, "admin.workflows.put")
            .await?;
        if self.workflow_binding_in(connection).await?.is_some() {
            return Err(fail(
                "shared_workflow",
                "edit the shared definition or detach this board first",
            )
            .into());
        }
        let current: Workflow = self.configuration(connection, "workflow").await?;
        if &current == workflow {
            return Ok(());
        }
        self.validate_replacement_in(connection, workflow).await?;
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
    }

    pub(super) async fn validate_replacement_in(
        &self,
        connection: &mut AnyConnection,
        workflow: &Workflow,
    ) -> DbResult<()> {
        let current: Workflow = self.configuration(connection, "workflow").await?;
        let states: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT state FROM board_tickets WHERE tenant_id=$1 AND board_id=$2",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .fetch_all(&mut *connection)
        .await?;
        for state in states {
            if workflow.state(&state).is_none() {
                return Err(invalid(format!(
                    "tickets are in state {state:?}, which the new workflow lacks"
                ))
                .into());
            }
            if current.kind(&state) != workflow.kind(&state) {
                return Err(invalid(format!(
                    "state {state:?} is used by tickets; keep its type and change its label instead"
                ))
                .into());
            }
        }
        Ok(())
    }

    /// Read both values and their versions in one scoped database snapshot.
    pub async fn configuration_state(&self) -> BridgeResult<ConfigurationState> {
        transaction!(&self.store, |connection| {
            self.scope(connection).await?;
            self.configuration_state_in(connection).await
        })
    }

    pub(super) async fn configuration_state_in(
        &self,
        connection: &mut AnyConnection,
    ) -> DbResult<ConfigurationState> {
        Ok(ConfigurationState {
            settings: self.configuration(connection, "settings").await?,
            settings_version: self.configuration_version(connection, "settings").await?,
            workflow: self.configuration(connection, "workflow").await?,
            workflow_version: self.configuration_version(connection, "workflow").await?,
        })
    }

    async fn configuration_version(
        &self,
        connection: &mut AnyConnection,
        key: &str,
    ) -> DbResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT version FROM board_settings WHERE tenant_id=$1 AND board_id=$2 AND key=$3",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(key)
        .fetch_one(&mut *connection)
        .await?)
    }

    pub(super) async fn expected_configuration(
        &self,
        connection: &mut AnyConnection,
        key: &str,
        expected: i64,
    ) -> DbResult<()> {
        let actual = self.configuration_version(connection, key).await?;
        if actual != expected {
            return Err(fail(
                "version_conflict",
                format!("{key} changed from version {expected} to {actual}; reload before saving"),
            )
            .into());
        }
        Ok(())
    }

    /// Edit the person's settings, preserving standing approvals in the same transaction.
    pub async fn edit_settings(
        &self,
        actor: &Actor<'_>,
        expected: i64,
        edit: &SettingsEdit,
    ) -> BridgeResult<ConfigurationState> {
        if !actor.human {
            return Err(fail(
                "humans_only",
                "only a person edits board coordination settings",
            ));
        }
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            self.expected_configuration(connection, "settings", expected)
                .await?;
            let current = self.configuration(connection, "settings").await?;
            let next = edit.apply(&current, now)?;
            self.replace_settings_in(connection, actor, now, &next)
                .await?;
            self.configuration_state_in(connection).await
        })
    }

    /// A versioned human change; no task is reinterpreted or orphaned by configuration.
    pub async fn edit_workflow(
        &self,
        actor: &Actor<'_>,
        expected: i64,
        workflow: &Workflow,
    ) -> BridgeResult<ConfigurationState> {
        if !actor.human {
            return Err(fail(
                "humans_only",
                "only a person edits the board workflow",
            ));
        }
        workflow.validate().map_err(invalid)?;
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            self.expected_configuration(connection, "workflow", expected)
                .await?;
            self.replace_workflow_in(connection, actor, now, workflow)
                .await?;
            self.configuration_state_in(connection).await
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
