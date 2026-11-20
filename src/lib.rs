//! AgentHub (`devagotchi-agenthub`): the agents' board. Tickets any subscription claims under a
//! lease, works, annotates and hands off, in Postgres (`hosted`, tables by `migrations`): a
//! project's standalone board under a local tenant (`project.rs`), or a hub's; notes that reach
//! agents (`inbox`, `wake`), approvals (`approval`), GitHub issues (`issue_lifecycle`)
//! and the `board` command line (`cli`). Its Postgres engine is `devagotchi-store`; the hub's MCP
//! tools, tenant registry and pages are the hub's (`crates/hub/src/agenthub/`). Contract:
//! `docs/agent-board.md`. A ticket coordinates and remembers; a broker operation executes.

mod anchors;
pub mod approval;
mod board;
mod chain;
mod checks;
pub mod cli;
pub mod clickhouse;
mod clock;
pub mod face;
pub mod hosted;
pub mod inbox;
pub mod issue_lifecycle;
mod lease;
pub mod migrations;
mod project;
mod provenance;
pub mod scan;
mod settings;
mod ticket;
pub mod wake;
pub mod welcome;
pub mod workflow;

pub use anchors::{AnchorPatch, Anchors};
pub use board::{Board, Held, NoteToMe, PROTOCOL_LINE, Prime, Ready};
pub use chain::{GENESIS, actions, event_hash};
pub use checks::{fail, valid_name};
pub use clock::{Clock, system_clock, utc};
pub use lease::{Lease, NextFilter};
pub use project::*;
pub use provenance::{TicketProvenance, TicketSource};
pub use settings::{ConfigurationState, ProjectSettings, Settings, SettingsEdit};
pub use ticket::{
    Actor, ChainStatus, Dep, DepKind, Event, EventFilter, Filter, NewTicket, Note, Patch, Ticket,
    TicketView, UNDO_MS,
};
pub use workflow::{StateKind, Workflow};

/// The board layout a board's metadata names (`schema_version`): 2 since tickets carry
/// anchors. The tables themselves are `migrations/`.
const SCHEMA_VERSION: i64 = 2;

use anchors::clean_anchor;
use checks::invalid;
use settings::{check_environment, check_environments, check_repo};
use ticket::clean_labels;
use workflow::check_transition;

#[cfg(test)]
mod tests;
