//! The standalone board's SQLite schema, its version and the upgrades from earlier boards.

use std::cell::Cell;

use devagotchi_contracts::BridgeResult;
use rusqlite::{Connection, TransactionBehavior, params};

use crate::{Workflow, Writer, load_workflow, store_error};

pub const SCHEMA_VERSION: i64 = 2;

pub(crate) const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS fence (id INTEGER PRIMARY KEY CHECK (id = 1), value INTEGER NOT NULL);
INSERT OR IGNORE INTO fence (id, value) VALUES (1, 0);
CREATE TABLE IF NOT EXISTS tickets (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL DEFAULT 'task',
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 2,
  labels TEXT NOT NULL DEFAULT '[]',
  parent TEXT REFERENCES tickets(id),
  child_seq INTEGER NOT NULL DEFAULT 0,
  assignee TEXT,
  holder TEXT,
  lease_id TEXT,
  lease_ttl_ms INTEGER,
  lease_expires_at INTEGER,
  heartbeat_at INTEGER,
  fence INTEGER NOT NULL DEFAULT 0,
  version INTEGER NOT NULL DEFAULT 1,
  expiry_count INTEGER NOT NULL DEFAULT 0,
  last_worker TEXT,
  progress TEXT,
  created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  closed_at INTEGER,
  machine TEXT,
  repo TEXT,
  environment TEXT,
  agent TEXT,
  session TEXT,
  fingerprint TEXT,
  occurrences INTEGER NOT NULL DEFAULT 1,
  last_seen_at INTEGER,
  CHECK ((holder IS NULL) = (lease_id IS NULL))
);
CREATE INDEX IF NOT EXISTS tickets_queue ON tickets (state, priority, created_at) WHERE holder IS NULL;
CREATE INDEX IF NOT EXISTS tickets_lease ON tickets (lease_expires_at) WHERE holder IS NOT NULL;
CREATE INDEX IF NOT EXISTS tickets_parent ON tickets (parent);
CREATE TABLE IF NOT EXISTS deps (
  ticket TEXT NOT NULL REFERENCES tickets(id),
  depends_on TEXT NOT NULL REFERENCES tickets(id),
  kind TEXT NOT NULL DEFAULT 'blocks',
  created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (ticket, depends_on, kind),
  CHECK (ticket <> depends_on)
);
CREATE INDEX IF NOT EXISTS deps_reverse ON deps (depends_on);
CREATE TABLE IF NOT EXISTS notes (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  ticket TEXT NOT NULL REFERENCES tickets(id),
  author TEXT NOT NULL,
  kind TEXT NOT NULL,
  body TEXT NOT NULL,
  to_principal TEXT,
  fence INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS notes_ticket ON notes (ticket, seq);
CREATE TABLE IF NOT EXISTS events (
  seq INTEGER PRIMARY KEY,
  at INTEGER NOT NULL,
  actor TEXT NOT NULL,
  action TEXT NOT NULL,
  ticket TEXT,
  data TEXT NOT NULL DEFAULT '{}',
  fence INTEGER,
  prev_hash TEXT NOT NULL,
  hash TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS events_ticket ON events (ticket, seq);
CREATE TRIGGER IF NOT EXISTS events_no_update BEFORE UPDATE ON events
  BEGIN SELECT RAISE(ABORT, 'events is append-only'); END;
CREATE TRIGGER IF NOT EXISTS events_no_delete BEFORE DELETE ON events
  BEGIN SELECT RAISE(ABORT, 'events is append-only'); END;
CREATE TRIGGER IF NOT EXISTS notes_no_update BEFORE UPDATE ON notes
  BEGIN SELECT RAISE(ABORT, 'notes is append-only'); END;
CREATE TRIGGER IF NOT EXISTS notes_no_delete BEFORE DELETE ON notes
  BEGIN SELECT RAISE(ABORT, 'notes is append-only'); END;
";

/// Schema 2's indexes on the anchors, for new boards and the migration alike.
pub(crate) const ANCHOR_INDEXES: &str = "
CREATE INDEX IF NOT EXISTS tickets_machine ON tickets (machine);
CREATE INDEX IF NOT EXISTS tickets_environment ON tickets (environment);
CREATE INDEX IF NOT EXISTS tickets_fingerprint ON tickets (fingerprint);
";

/// Schema 1 -> 2 in one transaction: the anchor columns, their indexes, the version,
/// and a `board.migrated` event so the chain records it. Every row is kept.
/// A board that still runs the previous default workflow, exactly, gets the current one
/// (closed tickets reopen to the queue, the phone app's Undo), recorded on the chain as
/// `board.workflow_upgraded`. A workflow anyone changed is theirs and is left alone.
pub(crate) fn upgrade_default_workflow(connection: &mut Connection, now: i64) -> BridgeResult<()> {
    let previous = Workflow::previous_default();
    if load_workflow(connection)? != previous {
        return Ok(());
    }
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_error)?;
    // Checked again under the write lock: another process may have upgraded it first.
    if load_workflow(&tx)? != previous {
        return Ok(());
    }
    let current = Workflow::default_workflow();
    tx.execute(
        "UPDATE settings SET value = ?1 WHERE key = 'workflow'",
        params![serde_json::to_string(&current).expect("workflow json")],
    )
    .map_err(store_error)?;
    let appended = Cell::new(false);
    Writer {
        tx: &tx,
        actor: "system",
        now,
        appended: &appended,
    }
    .event(
        "board.workflow_upgraded",
        None,
        None,
        serde_json::json!({
            "id": current.id,
            "added": [{ "from": "done", "to": "ready" }, { "from": "cancelled", "to": "ready" }],
        }),
    )?;
    tx.commit().map_err(store_error)
}

pub(crate) fn migrate_v1(connection: &mut Connection, now: i64) -> BridgeResult<()> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_error)?;
    tx.execute_batch(
        "ALTER TABLE tickets ADD COLUMN machine TEXT;
         ALTER TABLE tickets ADD COLUMN repo TEXT;
         ALTER TABLE tickets ADD COLUMN environment TEXT;
         ALTER TABLE tickets ADD COLUMN agent TEXT;
         ALTER TABLE tickets ADD COLUMN session TEXT;
         ALTER TABLE tickets ADD COLUMN fingerprint TEXT;
         ALTER TABLE tickets ADD COLUMN occurrences INTEGER NOT NULL DEFAULT 1;
         ALTER TABLE tickets ADD COLUMN last_seen_at INTEGER;",
    )
    .map_err(store_error)?;
    tx.execute_batch(ANCHOR_INDEXES).map_err(store_error)?;
    tx.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
        params![SCHEMA_VERSION.to_string()],
    )
    .map_err(store_error)?;
    let appended = Cell::new(false);
    Writer {
        tx: &tx,
        actor: "system",
        now,
        appended: &appended,
    }
    .event(
        "board.migrated",
        None,
        None,
        serde_json::json!({ "from": 1, "to": SCHEMA_VERSION }),
    )?;
    tx.commit().map_err(store_error)
}
