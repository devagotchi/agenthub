use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub board_id: String,
    pub id: String,
    pub name: String,
    pub version: i64,
    pub retired_at: Option<i64>,
    pub workflow_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectEdit {
    pub id: String,
    pub name: String,
    /// Zero creates; positive versions update the selected active generation.
    pub version: i64,
    pub workflow_id: Option<String>,
    pub workflow_version: Option<i64>,
}

impl ProjectEdit {
    pub(super) fn check(&self) -> devagotchi_contracts::BridgeResult<()> {
        if !crate::valid_name(&self.id)
            || self.name.trim().is_empty()
            || self.name.chars().count() > 100
            || self.name.chars().any(char::is_control)
            || self.version < 0
            || self.workflow_id.is_some() != self.workflow_version.is_some()
            || self
                .workflow_id
                .as_ref()
                .is_some_and(|id| !crate::valid_name(id))
            || self.workflow_version.is_some_and(|version| version <= 0)
        {
            return Err(crate::invalid(
                "invalid project name, version or workflow selection",
            ));
        }
        Ok(())
    }
}
