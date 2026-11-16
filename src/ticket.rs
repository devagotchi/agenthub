//! A ticket and what hangs off it: who acts, dependencies, notes, events, and the shapes a create, patch or list takes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AnchorPatch, Anchors};

/// Who is acting. Identity comes from the hub's authentication, never from a tool
/// argument; `human` gates force-release; `wip_limit` overrides the board default.
#[derive(Debug, Clone)]
pub struct Actor<'a> {
    pub id: &'a str,
    pub human: bool,
    pub wip_limit: Option<i64>,
}

impl<'a> Actor<'a> {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn agent(id: &'a str) -> Self {
        Self {
            id,
            human: false,
            wip_limit: None,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn human(id: &'a str) -> Self {
        Self {
            id,
            human: true,
            wip_limit: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DepKind {
    /// `depends_on` must be done before the ticket is ready. The only kind readiness uses.
    Blocks,
    /// `depends_on` is the parent; the ticket is one of its children.
    Parent,
    Related,
    /// The ticket was found while working `depends_on` (agent-discovered work).
    DiscoveredFrom,
    Duplicates,
    Supersedes,
}

impl DepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DepKind::Blocks => "blocks",
            DepKind::Parent => "parent",
            DepKind::Related => "related",
            DepKind::DiscoveredFrom => "discovered-from",
            DepKind::Duplicates => "duplicates",
            DepKind::Supersedes => "supersedes",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "blocks" => DepKind::Blocks,
            "parent" => DepKind::Parent,
            "related" => DepKind::Related,
            "discovered-from" => DepKind::DiscoveredFrom,
            "duplicates" => DepKind::Duplicates,
            "supersedes" => DepKind::Supersedes,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Dep {
    pub ticket: String,
    pub depends_on: String,
    pub kind: DepKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Ticket {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub description: String,
    pub state: String,
    pub priority: i64,
    pub labels: Vec<String>,
    pub parent: Option<String>,
    pub assignee: Option<String>,
    pub holder: Option<String>,
    pub lease_id: Option<String>,
    pub lease_ttl_ms: Option<i64>,
    pub lease_expires_at: Option<i64>,
    pub heartbeat_at: Option<i64>,
    pub fence: i64,
    pub version: i64,
    pub expiry_count: i64,
    pub last_worker: Option<String>,
    pub progress: Option<serde_json::Value>,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub closed_at: Option<i64>,
    /// Where the ticket belongs (the project is the board it lives on). Each is
    /// optional and filled when known.
    #[serde(default)]
    pub machine: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    /// Folds repeats of one failure into this ticket (P3), counted in `occurrences`.
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default = "one")]
    pub occurrences: i64,
    #[serde(default)]
    pub last_seen_at: Option<i64>,
    /// Open `blocks` dependencies, computed on read.
    pub blocked_by: Vec<String>,
    /// The lease has lapsed but the reaper has not run yet, computed on read.
    pub lease_expired: bool,
}

impl Ticket {
    /// Where it belongs, as one line: `dabo · prod · github.com/x/y · claude (session …)`.
    /// `None` when nothing is known.
    pub fn anchor_line(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        parts.extend(self.machine.clone());
        parts.extend(self.environment.clone());
        if let Some(repo) = &self.repo {
            let bare = repo
                .trim_start_matches("https://")
                .trim_start_matches("http://");
            parts.push(
                bare.trim_end_matches(".git")
                    .trim_end_matches('/')
                    .to_string(),
            );
        }
        let session = self.session.as_deref().map(|s| {
            let short: String = s.chars().take(8).collect();
            if short.len() < s.len() {
                format!("{short}…")
            } else {
                short
            }
        });
        match (&self.agent, session) {
            (Some(agent), Some(session)) => parts.push(format!("{agent} (session {session})")),
            (Some(agent), None) => parts.push(agent.clone()),
            (None, Some(session)) => parts.push(format!("session {session}")),
            (None, None) => {}
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Note {
    pub seq: i64,
    pub ticket: String,
    pub author: String,
    pub kind: String,
    pub body: String,
    pub to_principal: Option<String>,
    pub fence: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    pub seq: i64,
    pub at: i64,
    pub actor: String,
    pub action: String,
    pub ticket: Option<String>,
    pub data: serde_json::Value,
    pub fence: Option<i64>,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TicketView {
    pub ticket: Ticket,
    pub deps: Vec<Dep>,
    pub dependents: Vec<Dep>,
    pub children: Vec<String>,
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChainStatus {
    pub entries: u64,
    pub ok: bool,
    pub head: String,
    pub first_broken: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct NewTicket {
    pub title: String,
    pub description: String,
    pub kind: Option<String>,
    pub priority: Option<i64>,
    pub labels: Vec<String>,
    /// Makes the id hierarchical (`parent.n`) and records a `parent` dependency.
    pub parent: Option<String>,
    pub assignee: Option<String>,
    /// Records a `discovered-from` dependency.
    pub discovered_from: Option<String>,
    pub anchors: Anchors,
    /// Defaults to 1.
    pub occurrences: Option<i64>,
    pub last_seen_at: Option<i64>,
}

pub(crate) fn one() -> i64 {
    1
}

#[derive(Debug, Clone, Default)]
pub struct Patch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<i64>,
    pub labels: Option<Vec<String>>,
    /// `Some(None)` clears the assignee.
    pub assignee: Option<Option<String>>,
    /// A workflow transition for an unheld ticket; held tickets move only through lease operations.
    pub state: Option<String>,
    pub anchors: AnchorPatch,
    pub occurrences: Option<i64>,
    /// `Some(None)` clears it.
    pub last_seen_at: Option<Option<i64>>,
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub state: Option<String>,
    pub holder: Option<String>,
    pub assignee: Option<String>,
    pub label: Option<String>,
    pub parent: Option<String>,
    pub kind: Option<String>,
    /// Only tickets that are claimable now (a lane queue, no open blockers, unheld or lapsed).
    pub ready: bool,
    pub open_only: bool,
    pub text: Option<String>,
    /// Equality on each anchor and the fingerprint.
    pub anchors: Anchors,
    pub limit: Option<usize>,
}

pub(crate) fn clean_labels(labels: &[String]) -> Vec<String> {
    let mut out: Vec<String> = labels
        .iter()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}
