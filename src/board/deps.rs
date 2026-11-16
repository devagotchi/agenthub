//! Dependencies between tickets: adding one without a cycle and removing it.

use devagotchi_contracts::BridgeResult;
use rusqlite::params;

use crate::{
    Actor, Board, Dep, DepKind, fail, insert_dep, invalid, load_workflow, reaches, read_deps,
    read_ticket, store_error,
};

impl Board {
    pub fn add_dep(
        &self,
        actor: &Actor<'_>,
        id: &str,
        depends_on: &str,
        kind: DepKind,
    ) -> BridgeResult<Vec<Dep>> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.add_dep(actor, id, depends_on, kind), true);
        }
        if id == depends_on {
            return Err(invalid("a ticket cannot depend on itself"));
        }
        self.write(actor, |w| {
            let workflow = load_workflow(w.tx)?;
            let now = w.now;
            read_ticket(w.tx, &workflow, id, now)?;
            read_ticket(w.tx, &workflow, depends_on, now)?;
            if kind == DepKind::Blocks && reaches(w.tx, depends_on, id)? {
                return Err(fail(
                    "dependency_cycle",
                    format!("{depends_on} already depends on {id}"),
                ));
            }
            insert_dep(w.tx, id, depends_on, kind, actor.id, now)?;
            w.event(
                "dep.added",
                Some(id),
                None,
                serde_json::json!({ "depends_on": depends_on, "kind": kind.as_str() }),
            )?;
            read_deps(w.tx, "ticket", id)
        })
    }

    pub fn remove_dep(
        &self,
        actor: &Actor<'_>,
        id: &str,
        depends_on: &str,
        kind: DepKind,
    ) -> BridgeResult<Vec<Dep>> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.remove_dep(actor, id, depends_on, kind), true);
        }
        self.write(actor, |w| {
            let removed =
                w.tx.execute(
                    "DELETE FROM deps WHERE ticket = ?1 AND depends_on = ?2 AND kind = ?3",
                    params![id, depends_on, kind.as_str()],
                )
                .map_err(store_error)?;
            if removed == 0 {
                return Err(fail("not_found", "no such dependency"));
            }
            w.event(
                "dep.removed",
                Some(id),
                None,
                serde_json::json!({ "depends_on": depends_on, "kind": kind.as_str() }),
            )?;
            read_deps(w.tx, "ticket", id)
        })
    }
}
