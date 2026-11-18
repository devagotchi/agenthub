//! The protocol, exercised the way the Go hub's `hubtest` did: fencing, expiry,
//! the distinct-reviewer lane, readiness through dependencies, hierarchy, the chain. Every
//! board here is a standalone board (`project.rs`) in a database of its own on the test
//! Postgres (`DEVAGOTCHI_TEST_POSTGRES_URL`); without one, each test skips (`or_skip!`).

pub(crate) use devagotchi_store::or_skip;
use devagotchi_store::transaction;

use crate::{
    Actor, AnchorPatch, Anchors, Board, DepKind, Filter, LOCAL_TENANT, NewTicket, NextFilter,
    Patch, Settings, StateKind, Workflow,
};

/// The project every fixture's board is.
pub(crate) const PROJECT: &str = "devagotchi";

pub(crate) struct Fixture {
    pub(crate) board: Board,
    pub(crate) test: devagotchi_store::testing::TestStore,
}

/// A standalone board, `devagotchi`, made in a new database the way `board init` makes one,
/// or `None` without a test Postgres. Its synchronous calls want a multi-thread runtime:
/// `#[tokio::test(flavor = "multi_thread")]`.
pub(crate) async fn fixture() -> Option<Fixture> {
    let test = postgres().await?;
    let board = crate::open_board(&test.url, PROJECT, true).await.unwrap();
    Some(Fixture { board, test })
}

impl Fixture {
    /// Let `ms` pass for what the board keeps time on: every lease lapses `ms` sooner, and
    /// every standing authorization was granted, ends and was revoked `ms` earlier. The board
    /// reads Postgres's clock, so the rows move instead of the clock; nothing lands on the
    /// chain.
    pub(crate) fn advance(&self, ms: i64) {
        let store = &self.test.store;
        self.board
            .hosted()
            .blocking(async move {
                transaction!(store, |connection| {
                    store.scope(connection, LOCAL_TENANT).await?;
                    sqlx::query(
                        "UPDATE board_tickets SET lease_expires_at=lease_expires_at-$3
                         WHERE tenant_id=$1 AND board_id=$2 AND lease_expires_at IS NOT NULL",
                    )
                    .bind(LOCAL_TENANT)
                    .bind(PROJECT)
                    .bind(ms)
                    .execute(&mut *connection)
                    .await?;
                    let raw: String = sqlx::query_scalar(
                        "SELECT value FROM board_settings
                         WHERE tenant_id=$1 AND board_id=$2 AND key='settings'",
                    )
                    .bind(LOCAL_TENANT)
                    .bind(PROJECT)
                    .fetch_one(&mut *connection)
                    .await?;
                    let mut settings: Settings = serde_json::from_str(&raw)
                        .map_err(|error| crate::fail("test", error.to_string()))?;
                    for standing in &mut settings.standing {
                        standing.granted_at -= ms;
                        standing.expires_at -= ms;
                        if let Some(revoked_at) = &mut standing.revoked_at {
                            *revoked_at -= ms;
                        }
                    }
                    sqlx::query(
                        "UPDATE board_settings SET value=$3
                         WHERE tenant_id=$1 AND board_id=$2 AND key='settings'",
                    )
                    .bind(LOCAL_TENANT)
                    .bind(PROJECT)
                    .bind(serde_json::to_string(&settings).unwrap())
                    .execute(&mut *connection)
                    .await?;
                    Ok(())
                })
            })
            .unwrap();
    }

    pub(crate) fn new_ticket(&self, actor: &Actor<'_>, title: &str) -> String {
        self.board
            .create(
                actor,
                NewTicket {
                    title: title.into(),
                    ..NewTicket::default()
                },
            )
            .expect("create")
            .id
    }
}

mod anchors;
mod approvals;
mod hosted;
mod hosted_lifecycle;
mod inbox;
mod lanes;
mod schema;
mod sources;
mod standalone;
mod standing;
mod updates;

/// A new database on the test Postgres with AgentHub's tables, or `None` without one.
pub(super) async fn postgres() -> Option<devagotchi_store::testing::TestStore> {
    devagotchi_store::testing::postgres_with(&[&crate::migrations::MIGRATIONS]).await
}

/// The stores a test runs on: [`postgres`], or none.
pub(super) async fn stores() -> Vec<devagotchi_store::testing::TestStore> {
    postgres().await.into_iter().collect()
}

/// The board change channel a hosted board notifies, heard on `url`.
pub(super) async fn board_changes(url: &str) -> sqlx::postgres::PgListener {
    let mut listener = sqlx::postgres::PgListener::connect(url).await.unwrap();
    listener.listen("devagotchi_board_changes").await.unwrap();
    listener
}
