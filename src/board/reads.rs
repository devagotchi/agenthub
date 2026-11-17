//! Reading a board: one ticket, a filtered list, the events after a point, and the chain's check.

use devagotchi_contracts::BridgeResult;
use rusqlite::params;

use crate::{
    Board, ChainStatus, Event, Filter, GENESIS, Ticket, TicketView, event_from_row, event_hash,
    list_tickets, load_workflow, read_deps, read_notes, read_ticket, store_error,
};

impl Board {
    pub fn get(&self, id: &str) -> BridgeResult<TicketView> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.view(id), false);
        }
        let connection = self.local()?;
        let workflow = load_workflow(&connection)?;
        let ticket = read_ticket(&connection, &workflow, id, self.now())?;
        let deps = read_deps(&connection, "ticket", id)?;
        let dependents = read_deps(&connection, "depends_on", id)?;
        let children = {
            let mut statement = connection
                .prepare("SELECT id FROM tickets WHERE parent = ?1 ORDER BY created_at, id")
                .map_err(store_error)?;
            let rows = statement
                .query_map(params![id], |row| row.get::<_, String>(0))
                .map_err(store_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(store_error)?
        };
        let notes = read_notes(&connection, id)?;
        Ok(TicketView {
            ticket,
            deps,
            dependents,
            children,
            notes,
        })
    }

    pub fn list(&self, filter: &Filter) -> BridgeResult<Vec<Ticket>> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.list(filter), false);
        }
        let connection = self.local()?;
        let workflow = load_workflow(&connection)?;
        list_tickets(&connection, &workflow, filter, self.now(), None)
    }

    pub fn events(&self, after_seq: i64, limit: usize) -> BridgeResult<Vec<Event>> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.events_page(after_seq, limit), false);
        }
        let connection = self.local()?;
        let mut statement = connection
            .prepare(
                "SELECT seq, at, actor, action, ticket, data, fence, prev_hash, hash
                 FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
            )
            .map_err(store_error)?;
        let rows = statement
            .query_map(params![after_seq, limit.max(1) as i64], event_from_row)
            .map_err(store_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(store_error)
    }

    /// Recompute the event chain. The first broken seq, if any, is reported, never hidden.
    pub fn verify_chain(&self) -> BridgeResult<ChainStatus> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.verify_chain(), false);
        }
        let connection = self.local()?;
        let mut statement = connection
            .prepare(
                "SELECT seq, at, actor, action, ticket, data, fence, prev_hash, hash FROM events ORDER BY seq",
            )
            .map_err(store_error)?;
        let rows = statement
            .query_map([], event_from_row)
            .map_err(store_error)?;
        let mut prev = GENESIS.to_string();
        let mut entries = 0u64;
        let mut first_broken = None;
        let mut head = GENESIS.to_string();
        for event in rows {
            let event = event.map_err(store_error)?;
            entries += 1;
            let expected = event_hash(&prev, &event);
            if first_broken.is_none() && (event.prev_hash != prev || event.hash != expected) {
                first_broken = Some(event.seq);
            }
            prev = event.hash.clone();
            head = event.hash;
        }
        Ok(ChainStatus {
            entries,
            ok: first_broken.is_none(),
            head,
            first_broken,
        })
    }

    // ----- writes ------------------------------------------------------------
}
