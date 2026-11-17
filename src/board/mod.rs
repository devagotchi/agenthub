//! A board: one project's tickets, standalone in SQLite or hosted in Postgres, and the
//! accessors and write path every operation shares.

use std::{
    cell::Cell,
    path::Path,
    sync::{Arc, Mutex},
};

use devagotchi_contracts::BridgeResult;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use tokio::sync::{Notify, watch};

use crate::{
    Actor, Clock, SCHEMA, SCHEMA_VERSION, Settings, Workflow, Writer, default_prefix, fail, hosted,
    invalid, local::ANCHOR_INDEXES, migrate_v1, store_error, system_clock,
    upgrade_default_workflow,
};

mod deps;
mod prime;
mod reads;
mod settings;
mod tickets;

pub use prime::{Held, NoteToMe, Prime, Ready};

/// One project's board. One connection, one writer: SQLite in WAL mode serialises
/// writers anyway, and every mutation appends its event in the same transaction.
pub struct Board {
    pub(crate) connection: Option<Mutex<Connection>>,
    pub(crate) hosted: Option<hosted::HostedBoard>,
    pub(crate) project: String,
    pub(crate) prefix: String,
    pub(crate) clock: Clock,
    /// Advances each time a write commits an event, so a wait (board_wait, approval_wait)
    /// sleeps until the board changes instead of reading it again and again.
    pub(crate) changes: watch::Sender<u64>,
    /// Also told of every committed change: the hub's lease reaper, which sleeps until the
    /// next lease lapses or the leases change.
    pub(crate) activity: Option<Arc<Notify>>,
}

/// The board's protocol in one line: what every prime returns and the command line prints.
pub const PROTOCOL_LINE: &str =
    "claim → beat → note → done (or handoff with a note). Never stop holding a ticket.";

impl Board {
    /// Open (creating if needed) the board at `path` for `project`. The prefix is
    /// fixed at creation; reopening with another project name is refused, because
    /// ids embed it and a board never changes identity by being moved.
    pub fn open(path: &Path, project: &str) -> BridgeResult<Self> {
        Self::open_with_clock(path, project, system_clock())
    }

    pub fn open_with_clock(path: &Path, project: &str, clock: Clock) -> BridgeResult<Self> {
        if project.trim().is_empty() {
            return Err(invalid("project name is required"));
        }
        let connection = Connection::open(path).map_err(store_error)?;
        connection.execute_batch(SCHEMA).map_err(store_error)?;
        let stored: Option<String> = connection
            .query_row("SELECT value FROM meta WHERE key = 'project'", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(store_error)?;
        let prefix = match stored {
            Some(existing) if existing != project => {
                return Err(fail(
                    "board_mismatch",
                    format!("this board belongs to project {existing:?}, not {project:?}"),
                ));
            }
            Some(_) => connection
                .query_row("SELECT value FROM meta WHERE key = 'prefix'", [], |row| {
                    row.get(0)
                })
                .map_err(store_error)?,
            None => {
                let prefix = default_prefix(project);
                let now = clock();
                let workflow = Workflow::default_workflow();
                connection
                    .execute_batch("BEGIN IMMEDIATE")
                    .map_err(store_error)?;
                connection
                    .execute_batch(ANCHOR_INDEXES)
                    .map_err(store_error)?;
                for (key, value) in [
                    ("schema_version", SCHEMA_VERSION.to_string()),
                    ("project", project.to_string()),
                    ("prefix", prefix.clone()),
                    ("created_at", now.to_string()),
                ] {
                    connection
                        .execute(
                            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                            params![key, value],
                        )
                        .map_err(store_error)?;
                }
                connection
                    .execute(
                        "INSERT INTO settings (key, value) VALUES ('settings', ?1), ('workflow', ?2)",
                        params![
                            serde_json::to_string(&Settings::default()).expect("settings json"),
                            serde_json::to_string(&workflow).expect("workflow json"),
                        ],
                    )
                    .map_err(store_error)?;
                connection.execute_batch("COMMIT").map_err(store_error)?;
                prefix
            }
        };
        let mut connection = connection;
        let mut version: String = connection
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .map_err(store_error)?;
        if version == "1" {
            migrate_v1(&mut connection, clock())?;
            version = SCHEMA_VERSION.to_string();
        }
        if version != SCHEMA_VERSION.to_string() {
            return Err(fail(
                "board_version",
                format!("board schema {version} is not {SCHEMA_VERSION}; migrate before use"),
            ));
        }
        upgrade_default_workflow(&mut connection, clock())?;
        Ok(Self {
            connection: Some(Mutex::new(connection)),
            hosted: None,
            project: project.to_string(),
            prefix,
            clock,
            changes: watch::channel(0).0,
            activity: None,
        })
    }

    pub async fn from_hosted(hosted: hosted::HostedBoard) -> BridgeResult<Self> {
        let prefix = hosted.prefix().await?;
        Ok(Self {
            project: hosted.name.clone(),
            prefix,
            clock: system_clock(),
            connection: None,
            hosted: Some(hosted),
            changes: watch::channel(0).0,
            activity: None,
        })
    }

    pub fn hosted_backend(&self) -> BridgeResult<&hosted::HostedBoard> {
        self.hosted.as_ref().ok_or_else(|| {
            fail(
                "not_offered",
                "issue integration requires a hosted Postgres board",
            )
        })
    }

    /// Whether this board is a local SQLite file (not a hosted board).
    pub fn is_local(&self) -> bool {
        self.connection.is_some()
    }

    /// The local board's SQLite connection, for tests that need to reach under the API.
    #[doc(hidden)]
    pub fn local(&self) -> BridgeResult<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .as_ref()
            .ok_or_else(|| fail("board_store", "hosted board cannot access local storage"))?
            .lock()
            .map_err(|_| fail("board_store", "board lock poisoned"))
    }

    pub(crate) fn hosted_call<T>(
        &self,
        future: impl std::future::Future<Output = BridgeResult<T>>,
        _mutation: bool,
    ) -> BridgeResult<T> {
        let hosted = self
            .hosted
            .as_ref()
            .ok_or_else(|| fail("board_store", "no hosted backend"))?;
        // The Postgres listener reports committed events, including other hubs' writes.
        // Successful no-ops must not wake board_wait or the lease reaper.
        hosted.blocking(future)
    }

    /// Tell `activity` of every change this board commits from now on.
    pub fn reporting_to(mut self, activity: Arc<Notify>) -> Self {
        self.activity = Some(activity);
        self
    }

    /// A receiver that sees a new value after each committed change. Subscribe before
    /// reading, then await `changed()`: a change in between is never missed.
    /// The hosted board behind this one, when it is one.
    pub fn hosted(&self) -> Option<&hosted::HostedBoard> {
        self.hosted.as_ref()
    }

    /// Wake whoever waits on this board (`changes`): another hub instance committed to it.
    pub fn mark_changed(&self) {
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// When the first lease held on this board lapses, if any is held.
    pub fn next_lease_expiry(&self) -> BridgeResult<Option<i64>> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.next_lease_expiry(), false);
        }
        let connection = self.local()?;
        connection
            .query_row(
                "SELECT MIN(lease_expires_at) FROM tickets WHERE holder IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .map_err(store_error)
    }

    #[allow(dead_code)]
    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    /// Run one write transaction: IMMEDIATE so the event chain cannot fork.
    pub fn write<T>(
        &self,
        actor: &Actor<'_>,
        body: impl FnOnce(&mut Writer<'_>) -> BridgeResult<T>,
    ) -> BridgeResult<T> {
        let now = self.now();
        let mut connection = self.local()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(store_error)?;
        let appended = Cell::new(false);
        let result = {
            let mut writer = Writer {
                tx: &tx,
                actor: actor.id,
                now,
                appended: &appended,
            };
            body(&mut writer)
        };
        match result {
            Ok(value) => {
                tx.commit().map_err(store_error)?;
                drop(connection);
                // Only a write that recorded something wakes anyone: an empty reap does not.
                if appended.get() {
                    self.changes
                        .send_modify(|revision| *revision = revision.wrapping_add(1));
                    if let Some(activity) = &self.activity {
                        activity.notify_one();
                    }
                }
                Ok(value)
            }
            Err(error) => {
                // Dropping the transaction rolls it back.
                Err(error)
            }
        }
    }
}
