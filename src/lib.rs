//! AgentHub (`devagotchi-agenthub`): the agents' board. Tickets any subscription claims under a
//! lease, works, annotates and hands off: a standalone board in `<project>/.devagotchi/
//! board.sqlite3` (ADR 0002), a hosted one in Postgres (`hosted`, tables by `migrations`); notes
//! that reach agents (`inbox`, `wake`), approvals (`approval`), GitHub issues (`issue_lifecycle`)
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
mod local;
pub mod migrations;
mod project;
pub mod scan;
mod settings;
mod ticket;
pub mod wake;
pub mod welcome;
pub mod workflow;

pub use anchors::{AnchorPatch, Anchors};
pub use board::{Board, Held, NoteToMe, PROTOCOL_LINE, Prime, Ready};
pub use chain::{GENESIS, Writer, event_from_row, event_hash};
pub use checks::{fail, store_error, valid_name};
pub use clock::{Clock, system_clock, utc};
pub use lease::{Lease, NextFilter};
pub use local::{SCHEMA_VERSION, insert_note, load_settings, load_workflow, read_ticket};
pub use project::*;
pub use settings::{ProjectSettings, Settings};
pub use ticket::{
    Actor, ChainStatus, Dep, DepKind, Event, Filter, NewTicket, Note, Patch, Ticket, TicketView,
};
pub use workflow::{StateKind, Workflow};

use anchors::clean_anchor;
use checks::invalid;
use local::{
    SCHEMA, insert_dep, list_tickets, migrate_v1, note_from_row, open_blockers, reaches, read_deps,
    read_note, read_notes, ticket_from_row, upgrade_default_workflow,
};
use settings::{check_environment, check_environments, check_repo};
use ticket::clean_labels;
use workflow::check_transition;

#[cfg(test)]
mod tests;
