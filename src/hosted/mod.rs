//! A board in Postgres: its transactions, under the tenant and board id its caller resolved.
//! Each subject is a file (`.ai/patterns/board-operations.md`, Where files go): `tickets`,
//! `leases`, `notes`, `graph`, `approvals`, `standing`, `settings`, `inbox`, `issues`,
//! `reads`, `registry`. This file holds the board itself: how one is made and scoped, its clock,
//! its ids, reading a ticket and appending an event.

use devagotchi_contracts::BridgeResult;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use sqlx::{AnyConnection, Row};

use super::{
    Actor, Event, GENESIS, NewTicket, Note, Settings, Ticket, Workflow, check_environment,
    clean_anchor, clean_labels, default_prefix, event_hash, fail, invalid, lease::Lease,
};
use devagotchi_store::{Failed, Store, transaction};

mod access;
mod approvals;
pub use access::TaskLeaseBinding;
mod claims;
mod delegation;
mod edits;
mod events;
mod graph;
mod inbox;
mod issues;
mod lease_expiry;
mod leases;
mod notes;
mod policy;
mod projects;
pub use projects::{ProjectEdit, ProjectRecord};
mod reads;
mod receipts;
use receipts::Receipt;
mod recipients;
pub use recipients::PrincipalDirectory;
pub mod registry;
mod settings;
mod shared_workflows;
mod standing;
mod tickets;
mod updates;

#[derive(Clone)]
pub struct HostedBoard {
    store: Store,
    tenant: String,
    project: String,
    pub name: String,
    owner: Option<String>,
    runtime: tokio::runtime::Handle,
    principals: Option<std::sync::Arc<dyn PrincipalDirectory>>,
    mutation: Option<receipts::Mutation>,
}

type DbResult<T> = std::result::Result<T, Failed>;

impl HostedBoard {
    /// The tenant whose rows this board's are.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The board's id in its tenant: what its events carry (`board_id`).
    pub fn board_id(&self) -> &str {
        &self.project
    }
}

fn decode<T: DeserializeOwned>(text: &str) -> BridgeResult<T> {
    serde_json::from_str(text).map_err(|error| {
        fail(
            "board_damaged",
            format!("invalid stored board data: {error}"),
        )
    })
}

impl HostedBoard {
    /// Tenant/project must come from the authenticated resolver, never a tool payload.
    pub fn new(store: Store, tenant: &str, project: &str) -> BridgeResult<Self> {
        let internal = project.strip_prefix("b-").is_some_and(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        });
        if !devagotchi_store::valid_tenant_id(tenant) || (!internal && !crate::valid_name(project))
        {
            return Err(invalid("hosted board requires a valid tenant and project"));
        }
        Ok(Self {
            store,
            tenant: tenant.into(),
            project: project.into(),
            name: project.into(),
            owner: None,
            principals: None,
            mutation: None,
            runtime: tokio::runtime::Handle::try_current()
                .map_err(|error| fail("runtime", error.to_string()))?,
        })
    }

    pub fn for_owner(store: Store, tenant: &str, owner: &str, project: &str) -> BridgeResult<Self> {
        let mut board = Self::new(store, tenant, project)?;
        if owner.is_empty() || owner.len() > 256 {
            return Err(invalid("invalid board owner"));
        }
        board.project = format!(
            "b-{}",
            hex::encode(Sha256::digest(format!("{owner}\0{project}").as_bytes()))
        );
        board.owner = Some(owner.into());
        Ok(board)
    }

    // MCP/reaper use blocking workers; Tokio compensates for synchronous web callers.
    pub fn blocking<T>(
        &self,
        future: impl std::future::Future<Output = BridgeResult<T>>,
    ) -> BridgeResult<T> {
        blocking(&self.runtime, future)
    }

    pub async fn prefix(&self) -> BridgeResult<String> {
        transaction!(&self.store, |connection| {
            self.scope(connection).await?;
            Ok(sqlx::query_scalar(
                "SELECT value FROM board_metadata WHERE tenant_id=$1 AND board_id=$2 AND key='prefix'",
            ).bind(&self.tenant).bind(&self.project).fetch_one(&mut *connection).await?)
        })
    }

    pub async fn database_now(&self) -> BridgeResult<i64> {
        transaction!(&self.store, |connection| { self.scope(connection).await })
    }

    pub fn matches_notification(&self, tenant: &str, board: &str) -> bool {
        self.tenant == tenant && self.project == board
    }

    pub async fn initialize(&self) -> BridgeResult<bool> {
        self.initialize_limited(None).await
    }

    pub async fn initialize_limited(&self, limit: Option<usize>) -> BridgeResult<bool> {
        transaction!(&self.store, |connection| {
            self.initialize_in(connection, limit).await
        })
    }

    async fn server_now(connection: &mut AnyConnection) -> DbResult<i64> {
        Ok(
            sqlx::query_scalar(
                "SELECT floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint",
            )
            .fetch_one(&mut *connection)
            .await?,
        )
    }

    async fn scope(&self, connection: &mut AnyConnection) -> DbResult<i64> {
        self.store.scope(connection, &self.tenant).await?;
        // One database lock orders events, fences, WIP and graph changes across hub instances.
        let project: Option<String> = sqlx::query_scalar(
            "SELECT id FROM board_projects WHERE tenant_id=$1 AND id=$2 AND retired_at IS NULL FOR UPDATE",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .fetch_optional(&mut *connection)
        .await?;
        if project.is_none() {
            return Err(fail("not_found", "hosted board does not exist").into());
        }
        Self::server_now(connection).await
    }

    async fn configuration<T: DeserializeOwned>(
        &self,
        connection: &mut AnyConnection,
        key: &str,
    ) -> DbResult<T> {
        if key == "workflow"
            && let Some(text) = self.shared_workflow_value(connection).await?
        {
            return Ok(decode(&text)?);
        }
        let text: String = sqlx::query_scalar(
            "SELECT value FROM board_settings WHERE tenant_id=$1 AND board_id=$2 AND key=$3",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(key)
        .fetch_one(&mut *connection)
        .await?;
        Ok(decode(&text)?)
    }

    async fn fresh_id(&self, connection: &mut AnyConnection) -> DbResult<String> {
        let prefix: String = sqlx::query_scalar(
            "SELECT value FROM board_metadata WHERE tenant_id=$1 AND board_id=$2 AND key='prefix'",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .fetch_one(&mut *connection)
        .await?;
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|error| fail("board_random", error.to_string()))?;
        let suffix = hex::encode(random);
        for length in 4..=12 {
            let id = format!("{prefix}-{}", &suffix[..length]);
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM board_tickets WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(&id)
            .fetch_one(&mut *connection)
            .await?;
            if count == 0 {
                return Ok(id);
            }
        }
        Err(fail("board_id", "could not allocate a unique id").into())
    }

    async fn ticket(
        &self,
        connection: &mut AnyConnection,
        id: &str,
        workflow: &Workflow,
        now: i64,
    ) -> DbResult<Ticket> {
        let text: Option<String> = sqlx::query_scalar(
            "SELECT row_to_json(t)::text FROM board_tickets t
             WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(id)
        .fetch_optional(&mut *connection)
        .await?;
        let text = text.ok_or_else(|| fail("not_found", format!("ticket {id} not found")))?;
        let mut value: serde_json::Value = decode(&text)?;
        for field in ["labels", "progress"] {
            if let Some(text) = value[field].as_str() {
                value[field] = decode(text)?;
            }
        }
        value["blocked_by"] = serde_json::json!([]);
        value["lease_expired"] = serde_json::json!(
            value["lease_expires_at"]
                .as_i64()
                .is_some_and(|at| at <= now)
        );
        let mut ticket: Ticket = serde_json::from_value(value)
            .map_err(|error| fail("board_damaged", error.to_string()))?;
        let blockers = sqlx::query(
            "SELECT d.depends_on,t.state FROM board_deps d JOIN board_tickets t
               ON t.tenant_id=d.tenant_id AND t.board_id=d.board_id AND t.id=d.depends_on
             WHERE d.tenant_id=$1 AND d.board_id=$2 AND d.ticket=$3 AND d.kind='blocks'
             ORDER BY d.depends_on COLLATE \"C\"",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(id)
        .fetch_all(&mut *connection)
        .await?;
        for blocker in blockers {
            if !workflow.is_done(&blocker.try_get::<String, _>("state")?) {
                ticket.blocked_by.push(blocker.try_get("depends_on")?);
            }
        }
        Ok(ticket)
    }

    pub async fn get(&self, id: &str) -> BridgeResult<Ticket> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            self.ticket(connection, id, &workflow, now).await
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn event(
        &self,
        connection: &mut AnyConnection,
        actor: &str,
        now: i64,
        action: &str,
        ticket: Option<&str>,
        fence: Option<i64>,
        data: serde_json::Value,
    ) -> DbResult<()> {
        let last = sqlx::query(
            "SELECT seq,hash FROM board_events WHERE tenant_id=$1 AND board_id=$2 ORDER BY seq DESC LIMIT 1",
        ).bind(&self.tenant).bind(&self.project).fetch_optional(&mut *connection).await?;
        let (seq, prev) = match last {
            Some(row) => (
                row.try_get::<i64, _>("seq")? + 1,
                row.try_get::<String, _>("hash")?,
            ),
            None => (1, GENESIS.to_string()),
        };
        let mut event = Event {
            seq,
            at: now,
            actor: actor.into(),
            action: action.into(),
            ticket: ticket.map(str::to_string),
            data,
            fence,
            prev_hash: prev.clone(),
            hash: String::new(),
        };
        event.hash = event_hash(&prev, &event);
        sqlx::query(
            "INSERT INTO board_events (tenant_id,board_id,seq,at,actor,action,ticket,data,fence,prev_hash,hash)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        ).bind(&self.tenant).bind(&self.project).bind(seq).bind(now).bind(actor).bind(action)
            .bind(ticket).bind(event.data.to_string()).bind(fence).bind(prev).bind(&event.hash)
            .execute(&mut *connection).await?;
        sqlx::query(
            "INSERT INTO board_outbox (tenant_id,board_id,event_id,kind,payload,created_at)
             VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(format!("event:{seq}:{}", event.hash))
        .bind(action)
        .bind(serde_json::to_string(&event).expect("event json"))
        .bind(now)
        .execute(&mut *connection)
        .await?;
        sqlx::query("SELECT pg_notify('devagotchi_board_changes',$1)")
            .bind(serde_json::json!([self.tenant, self.project]).to_string())
            .execute(&mut *connection)
            .await?;
        Ok(())
    }
}

pub fn blocking<T>(
    runtime: &tokio::runtime::Handle,
    future: impl std::future::Future<Output = BridgeResult<T>>,
) -> BridgeResult<T> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| runtime.block_on(future))
        }
        Ok(_) => Err(fail(
            "board_runtime",
            "synchronous hosted board calls require a blocking worker or multi-thread runtime",
        )),
        Err(_) => runtime.block_on(future),
    }
}
