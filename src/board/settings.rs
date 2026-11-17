//! Reading and changing a board's settings, workflow and project anchors.

use devagotchi_contracts::BridgeResult;
use rusqlite::params;

use crate::{
    Actor, Board, ProjectSettings, Settings, Workflow, check_environments, check_repo, invalid,
    load_settings, load_workflow, store_error,
};

impl Board {
    pub fn settings(&self) -> BridgeResult<Settings> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.settings(), false);
        }
        let connection = self.local()?;
        load_settings(&connection)
    }

    pub fn workflow(&self) -> BridgeResult<Workflow> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.workflow(), false);
        }
        let connection = self.local()?;
        load_workflow(&connection)
    }

    /// Change the settings in one transaction: `edit` sees them as they are now and says
    /// what it changed, which lands on the chain as `action`. Never a stale overwrite of a
    /// change made meanwhile, as replacing them whole (`put_settings`) could be.
    pub fn edit_settings<F>(
        &self,
        actor: &Actor<'_>,
        action: &str,
        edit: F,
    ) -> BridgeResult<Settings>
    where
        F: Fn(&mut Settings) -> BridgeResult<serde_json::Value>,
    {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.edit_settings(actor, action, &edit), true);
        }
        self.write(actor, |w| {
            let mut settings = load_settings(w.tx)?;
            let data = edit(&mut settings)?;
            w.tx.execute(
                "UPDATE settings SET value = ?1 WHERE key = 'settings'",
                params![serde_json::to_string(&settings).expect("settings json")],
            )
            .map_err(store_error)?;
            w.event(action, None, None, data)?;
            Ok(settings)
        })
    }

    /// Replace the settings. Validated before it lands; recorded as an admin event.
    /// Admin verbs: the operator's `board settings` / `board workflow` commands and the
    /// web board (increment 10) call these; until then only the tests do.
    #[allow(dead_code)]
    pub fn put_settings(&self, actor: &Actor<'_>, settings: &Settings) -> BridgeResult<()> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.put_settings(actor, settings), true);
        }
        if settings.lease_min_ttl_ms <= 0
            || settings.lease_max_ttl_ms < settings.lease_min_ttl_ms
            || settings.lease_ttl_ms < settings.lease_min_ttl_ms
            || settings.lease_ttl_ms > settings.lease_max_ttl_ms
        {
            return Err(invalid(
                "lease ttl bounds must satisfy min <= default <= max, min > 0",
            ));
        }
        self.write(actor, |w| {
            w.tx.execute(
                "UPDATE settings SET value = ?1 WHERE key = 'settings'",
                params![serde_json::to_string(settings).expect("settings json")],
            )
            .map_err(store_error)?;
            w.event("admin.settings.put", None, None, serde_json::json!({}))?;
            Ok(())
        })
    }

    /// Name the project's repo and environments (`board_settings`, `board settings`): the
    /// fields given replace the board's, after the same checks on every face; the other
    /// settings stay. Nothing given changes nothing and returns the settings as they are.
    /// A change is on the chain as `board.settings_changed`, naming what changed.
    pub fn set_project(
        &self,
        actor: &Actor<'_>,
        patch: &ProjectSettings,
    ) -> BridgeResult<Settings> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.set_project(actor, patch), true);
        }
        let repo = patch.repo.as_deref().map(check_repo).transpose()?;
        let environments = patch
            .environments
            .as_deref()
            .map(check_environments)
            .transpose()?;
        if repo.is_none() && environments.is_none() {
            return self.settings();
        }
        self.write(actor, |w| {
            let mut settings = load_settings(w.tx)?;
            let mut changed = serde_json::Map::new();
            if let Some(repo) = &repo {
                settings.repo = Some(repo.clone());
                changed.insert("repo".into(), serde_json::json!(repo));
            }
            if let Some(environments) = &environments {
                settings.environments = environments.clone();
                changed.insert("environments".into(), serde_json::json!(environments));
            }
            w.tx.execute(
                "UPDATE settings SET value = ?1 WHERE key = 'settings'",
                params![serde_json::to_string(&settings).expect("settings json")],
            )
            .map_err(store_error)?;
            w.event(
                "board.settings_changed",
                None,
                None,
                serde_json::Value::Object(changed),
            )?;
            Ok(settings)
        })
    }

    /// Replace the workflow. Every existing ticket's state must still exist in it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn put_workflow(&self, actor: &Actor<'_>, workflow: &Workflow) -> BridgeResult<()> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.put_workflow(actor, workflow), true);
        }
        workflow.validate().map_err(invalid)?;
        self.write(actor, |w| {
            let mut statement =
                w.tx.prepare("SELECT DISTINCT state FROM tickets")
                    .map_err(store_error)?;
            let states = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(store_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(store_error)?;
            drop(statement);
            if let Some(unknown) = states.iter().find(|s| workflow.state(s).is_none()) {
                return Err(invalid(format!(
                    "tickets are in state {unknown:?}, which the new workflow lacks"
                )));
            }
            w.tx.execute(
                "UPDATE settings SET value = ?1 WHERE key = 'workflow'",
                params![serde_json::to_string(workflow).expect("workflow json")],
            )
            .map_err(store_error)?;
            w.event(
                "admin.workflow.put",
                None,
                None,
                serde_json::json!({ "id": workflow.id }),
            )?;
            Ok(())
        })
    }

    // ----- reads -------------------------------------------------------------

    pub fn issue_source(
        &self,
        id: &str,
    ) -> BridgeResult<Option<crate::issue_lifecycle::SourceIssue>> {
        match &self.hosted {
            Some(hosted) => self.hosted_call(hosted.issue_source(id), false),
            None => Ok(None),
        }
    }
}
