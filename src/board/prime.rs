//! What an agent reads at the start of a session: what it holds, what it could claim, what needs a person, and notes to it.

use devagotchi_contracts::BridgeResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Actor, Board, Filter, PROTOCOL_LINE, Ticket};

/// What an agent needs at the start of a session, in one read: what it holds (with
/// the fences), what it could claim, what needs a person, and notes addressed to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Prime {
    pub project: String,
    pub actor: String,
    pub human: bool,
    pub held: Vec<Held>,
    pub ready: Vec<Ready>,
    pub attention: Vec<Ready>,
    pub notes_to_me: Vec<NoteToMe>,
    /// Heartbeat a held ticket at least this often.
    pub heartbeat_every_ms: i64,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Held {
    pub id: String,
    pub title: String,
    pub state: String,
    pub fence: i64,
    pub version: i64,
    pub lease_expires_in_ms: i64,
    /// Where it belongs, as one line (`Ticket::anchor_line`); absent when nothing is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchors: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Ready {
    pub id: String,
    pub title: String,
    pub state: String,
    pub priority: i64,
    /// Where it belongs, as one line (`Ticket::anchor_line`); absent when nothing is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchors: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoteToMe {
    pub ticket: String,
    pub from: String,
    pub body: String,
}

impl Board {
    /// What `actor` could claim right now, best first: assigned to them, then priority, then age.
    pub fn ready(&self, actor: &Actor<'_>, limit: usize) -> BridgeResult<Vec<Ticket>> {
        self.hosted_call(self.hosted.ready(actor, limit))
    }

    /// The session-start read. `limit` caps the ready list; held tickets are never cut.
    pub fn prime(&self, actor: &Actor<'_>, limit: usize) -> BridgeResult<Prime> {
        let held = self.list(&Filter {
            holder: Some(actor.id.to_string()),
            ..Filter::default()
        })?;
        let ready = self.ready(actor, limit.clamp(1, 10))?;
        let attention = self.list(&Filter {
            state: Some(self.workflow()?.attention_state),
            limit: Some(5),
            ..Filter::default()
        })?;
        let mut notes_to_me = Vec::new();
        for ticket in self.list(&Filter {
            open_only: true,
            limit: Some(50),
            ..Filter::default()
        })? {
            let view = self.get(&ticket.id)?;
            for note in view.notes.iter().rev().take(3) {
                if note.to_principal.as_deref() == Some(actor.id) {
                    notes_to_me.push(NoteToMe {
                        ticket: ticket.id.clone(),
                        from: note.author.clone(),
                        body: note.body.clone(),
                    });
                }
            }
        }
        let now = self.hosted_call(self.hosted.database_now())?;
        let settings = self.settings()?;
        let summary = |t: &Ticket| Ready {
            id: t.id.clone(),
            title: t.title.clone(),
            state: t.state.clone(),
            priority: t.priority,
            anchors: t.anchor_line(),
        };
        Ok(Prime {
            project: self.project.clone(),
            actor: actor.id.to_string(),
            human: actor.human,
            held: held
                .iter()
                .map(|t| Held {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    state: t.state.clone(),
                    fence: t.fence,
                    version: t.version,
                    lease_expires_in_ms: t.lease_expires_at.map(|e| e - now).unwrap_or_default(),
                    anchors: t.anchor_line(),
                })
                .collect(),
            ready: ready.iter().map(summary).collect(),
            attention: attention.iter().map(summary).collect(),
            notes_to_me,
            heartbeat_every_ms: settings.heartbeat_every_ms(settings.lease_ttl_ms),
            protocol: PROTOCOL_LINE.to_string(),
        })
    }
}
