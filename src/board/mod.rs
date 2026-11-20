//! A board: one project's tickets in Postgres (`hosted`), and the synchronous face every
//! operation shares. A standalone project's board (`project.rs`) and a hub's boards are the
//! same thing under different tenants.

use devagotchi_contracts::BridgeResult;
use tokio::sync::watch;

use crate::hosted;

mod deps;
mod issues;
mod prime;
mod reads;
mod settings;
mod tickets;

pub use prime::{Held, NoteToMe, Prime, Ready};

/// One project's board: a hosted board in Postgres, called synchronously. Every mutation
/// appends its event in the same database transaction.
pub struct Board {
    pub(crate) hosted: hosted::HostedBoard,
    pub(crate) project: String,
    pub(crate) prefix: String,
    /// Advances each time another writer's change is reported (`mark_changed`), so a wait
    /// (board_wait, approval_wait) sleeps until the board changes instead of reading it
    /// again and again. The hub's Postgres listener reports every committed event.
    pub(crate) changes: watch::Sender<u64>,
}

/// The board's protocol in one line: what every prime returns and the command line prints.
pub const PROTOCOL_LINE: &str =
    "claim → beat → note → done (or handoff with a note). Never stop holding a ticket.";

impl Board {
    pub async fn from_hosted(hosted: hosted::HostedBoard) -> BridgeResult<Self> {
        let prefix = hosted.prefix().await?;
        Ok(Self {
            project: hosted.name.clone(),
            prefix,
            hosted,
            changes: watch::channel(0).0,
        })
    }

    /// The hosted board behind this one.
    pub fn hosted(&self) -> &hosted::HostedBoard {
        &self.hosted
    }

    /// Run one of the hosted board's operations to completion from synchronous code.
    pub(crate) fn hosted_call<T>(
        &self,
        future: impl std::future::Future<Output = BridgeResult<T>>,
    ) -> BridgeResult<T> {
        // The Postgres listener reports committed events, including other hubs' writes.
        // Successful no-ops must not wake board_wait or the lease reaper.
        self.hosted.blocking(future)
    }

    /// Wake whoever waits on this board (`changes`): a change was committed to it.
    pub fn mark_changed(&self) {
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    /// A receiver that sees a new value after each reported change. Subscribe before
    /// reading, then await `changed()`: a change in between is never missed.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// When the first lease held on this board lapses, if any is held.
    pub fn next_lease_expiry(&self) -> BridgeResult<Option<i64>> {
        self.hosted_call(self.hosted.next_lease_expiry())
    }

    #[allow(dead_code)]
    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }
}
