//! Sync face of the existing PostgreSQL shared workflow owner.
use crate::{Actor, Board, ConfigurationState, SharedWorkflow, Workflow, WorkflowConfiguration};
use devagotchi_contracts::BridgeResult;

impl Board {
    pub fn workflow_configuration(&self) -> BridgeResult<WorkflowConfiguration> {
        self.hosted_call(self.hosted.workflow_configuration())
    }

    /// Caller must authorize the entire board-owner namespace before using this catalog.
    pub fn shared_workflows(&self) -> BridgeResult<Vec<SharedWorkflow>> {
        self.hosted_call(self.hosted.shared_workflows())
    }

    pub fn create_shared_workflow(
        &self,
        actor: &Actor<'_>,
        workflow: &Workflow,
    ) -> BridgeResult<SharedWorkflow> {
        self.hosted_call(self.hosted.create_shared_workflow(actor, workflow))
    }

    pub fn publish_workflow(
        &self,
        actor: &Actor<'_>,
        board_version: i64,
        id: &str,
        name: &str,
    ) -> BridgeResult<SharedWorkflow> {
        self.hosted_call(self.hosted.publish_workflow(actor, board_version, id, name))
    }

    pub fn bind_workflow(
        &self,
        actor: &Actor<'_>,
        board_version: i64,
        id: &str,
        catalog_version: i64,
    ) -> BridgeResult<ConfigurationState> {
        self.hosted_call(
            self.hosted
                .bind_workflow(actor, board_version, id, catalog_version),
        )
    }

    pub fn detach_workflow(
        &self,
        actor: &Actor<'_>,
        board_version: i64,
    ) -> BridgeResult<ConfigurationState> {
        self.hosted_call(self.hosted.detach_workflow(actor, board_version))
    }

    pub fn edit_shared_workflow(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        workflow: &Workflow,
    ) -> BridgeResult<SharedWorkflow> {
        self.hosted_call(
            self.hosted
                .edit_shared_workflow(actor, id, version, workflow),
        )
    }

    pub fn delete_shared_workflow(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
    ) -> BridgeResult<()> {
        self.hosted_call(self.hosted.delete_shared_workflow(actor, id, version))
    }
}
