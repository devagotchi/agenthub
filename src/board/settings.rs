//! Reading and changing a board's settings, workflow and project anchors.

use devagotchi_contracts::BridgeResult;

use crate::{Actor, Board, ProjectSettings, Settings, Workflow};

impl Board {
    pub fn settings(&self) -> BridgeResult<Settings> {
        self.hosted_call(self.hosted.settings())
    }

    pub fn workflow(&self) -> BridgeResult<Workflow> {
        self.hosted_call(self.hosted.workflow())
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
        self.hosted_call(self.hosted.edit_settings(actor, action, &edit))
    }

    /// Replace the settings. Validated before it lands; recorded as an admin event.
    /// Admin verbs: the operator's `board settings` / `board workflow` commands and the
    /// web board (increment 10) call these; until then only the tests do.
    #[allow(dead_code)]
    pub fn put_settings(&self, actor: &Actor<'_>, settings: &Settings) -> BridgeResult<()> {
        self.hosted_call(self.hosted.put_settings(actor, settings))
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
        self.hosted_call(self.hosted.set_project(actor, patch))
    }

    /// Replace the workflow. Every existing ticket's state must still exist in it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn put_workflow(&self, actor: &Actor<'_>, workflow: &Workflow) -> BridgeResult<()> {
        self.hosted_call(self.hosted.put_workflow(actor, workflow))
    }

    pub fn issue_source(
        &self,
        id: &str,
    ) -> BridgeResult<Option<crate::issue_lifecycle::SourceIssue>> {
        self.hosted_call(self.hosted.issue_source(id))
    }
}
