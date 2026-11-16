//! The hash chain every board write extends: the writer that appends an event, the genesis link and the hash.

use devagotchi_contracts::BridgeResult;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use sha2::{Digest, Sha256};
use std::cell::Cell;

use crate::{Event, fail, store_error};

/// One mutation in flight.
pub struct Writer<'a> {
    pub tx: &'a Transaction<'a>,
    pub actor: &'a str,
    pub now: i64,
    /// Set when this transaction appends an event.
    pub(crate) appended: &'a Cell<bool>,
}

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

impl Writer<'_> {
    /// Append one event in this transaction, hash-chained to the last one.
    pub fn event(
        &self,
        action: &str,
        ticket: Option<&str>,
        fence: Option<i64>,
        data: serde_json::Value,
    ) -> BridgeResult<i64> {
        let last: Option<(i64, String)> = self
            .tx
            .query_row(
                "SELECT seq, hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(store_error)?;
        let (seq, prev) = last
            .map(|(seq, hash)| (seq + 1, hash))
            .unwrap_or((1, GENESIS.to_string()));
        let event = Event {
            seq,
            at: self.now,
            actor: self.actor.to_string(),
            action: action.to_string(),
            ticket: ticket.map(str::to_string),
            data,
            fence,
            prev_hash: prev.clone(),
            hash: String::new(),
        };
        let hash = event_hash(&prev, &event);
        self.tx
            .execute(
                "INSERT INTO events (seq, at, actor, action, ticket, data, fence, prev_hash, hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    seq,
                    event.at,
                    event.actor,
                    event.action,
                    event.ticket,
                    serde_json::to_string(&event.data).expect("event json"),
                    event.fence,
                    event.prev_hash,
                    hash,
                ],
            )
            .map_err(store_error)?;
        self.appended.set(true);
        Ok(seq)
    }

    /// The next fence: global, strictly increasing, never reused.
    pub fn next_fence(&self) -> BridgeResult<i64> {
        self.tx
            .query_row(
                "UPDATE fence SET value = value + 1 WHERE id = 1 RETURNING value",
                [],
                |row| row.get(0),
            )
            .map_err(store_error)
    }

    /// A hash id, `prefix-xxxx`, lengthened on collision so two machines creating
    /// tickets at once never fight over a counter.
    pub(crate) fn fresh_id(
        &self,
        prefix: &str,
        project: &str,
        title: &str,
        actor: &str,
    ) -> BridgeResult<String> {
        let mut salt = [0u8; 8];
        getrandom::fill(&mut salt).map_err(|error| fail("board_random", error.to_string()))?;
        let digest = hex::encode(Sha256::digest(
            format!(
                "{project}\0{title}\0{actor}\0{}\0{}",
                self.now,
                hex::encode(salt)
            )
            .as_bytes(),
        ));
        for length in 4..=12 {
            let candidate = format!("{prefix}-{}", &digest[..length]);
            let taken: bool = self
                .tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM tickets WHERE id = ?1)",
                    params![candidate],
                    |row| row.get(0),
                )
                .map_err(store_error)?;
            if !taken {
                return Ok(candidate);
            }
        }
        Err(fail("board_id", "could not allocate a unique id"))
    }
}

pub fn event_hash(prev: &str, event: &Event) -> String {
    // Canonical: fixed field order, no whitespace, the same bytes any verifier recomputes.
    let canonical = serde_json::json!({
        "seq": event.seq,
        "at": event.at,
        "actor": event.actor,
        "action": event.action,
        "ticket": event.ticket,
        "data": event.data,
        "fence": event.fence,
    });
    let mut hasher = Sha256::new();
    hasher.update(prev.as_bytes());
    hasher.update(
        serde_json::to_string(&canonical)
            .expect("canonical json")
            .as_bytes(),
    );
    hex::encode(hasher.finalize())
}

pub fn event_from_row(row: &Row<'_>) -> rusqlite::Result<Event> {
    let data: String = row.get(5)?;
    Ok(Event {
        seq: row.get(0)?,
        at: row.get(1)?,
        actor: row.get(2)?,
        action: row.get(3)?,
        ticket: row.get(4)?,
        data: serde_json::from_str(&data).unwrap_or(serde_json::Value::Null),
        fence: row.get(6)?,
        prev_hash: row.get(7)?,
        hash: row.get(8)?,
    })
}
