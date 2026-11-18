//! Dependencies between tickets: adding one without a cycle and removing it.

use devagotchi_contracts::BridgeResult;

use crate::{Actor, Board, Dep, DepKind};

impl Board {
    pub fn add_dep(
        &self,
        actor: &Actor<'_>,
        id: &str,
        depends_on: &str,
        kind: DepKind,
    ) -> BridgeResult<Vec<Dep>> {
        self.hosted_call(self.hosted.add_dep(actor, id, depends_on, kind))
    }

    pub fn remove_dep(
        &self,
        actor: &Actor<'_>,
        id: &str,
        depends_on: &str,
        kind: DepKind,
    ) -> BridgeResult<Vec<Dep>> {
        self.hosted_call(self.hosted.remove_dep(actor, id, depends_on, kind))
    }
}
