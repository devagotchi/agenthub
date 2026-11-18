//! Reading a board: one ticket, a filtered list, the events after a point, and the chain's check.

use devagotchi_contracts::BridgeResult;

use crate::{Board, ChainStatus, Event, Filter, Ticket, TicketView};

impl Board {
    pub fn get(&self, id: &str) -> BridgeResult<TicketView> {
        self.hosted_call(self.hosted.view(id))
    }

    pub fn list(&self, filter: &Filter) -> BridgeResult<Vec<Ticket>> {
        self.hosted_call(self.hosted.list(filter))
    }

    pub fn events(&self, after_seq: i64, limit: usize) -> BridgeResult<Vec<Event>> {
        self.hosted_call(self.hosted.events_page(after_seq, limit))
    }

    /// Recompute the event chain. The first broken seq, if any, is reported, never hidden.
    pub fn verify_chain(&self) -> BridgeResult<ChainStatus> {
        self.hosted_call(self.hosted.verify_chain())
    }
}
