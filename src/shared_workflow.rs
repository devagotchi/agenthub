//! Shared workflow projections. Identity and owner scope come from the resolved board.
use serde::{Deserialize, Serialize};

/// Board configuration and its shared binding from one locked snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowConfiguration {
    pub configuration: crate::ConfigurationState,
    pub shared_workflow: Option<WorkflowBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowBinding {
    pub id: String,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedWorkflow {
    pub workflow: crate::Workflow,
    pub version: i64,
    /// Project names, only within this catalog's owner namespace.
    pub used_by: Vec<String>,
}
