//! Roster totals and visibility use the same live leases as principal capacity.
use super::{or_skip, postgres};
use crate::{
    Actor, NewTicket,
    hosted::{
        HostedBoard,
        registry::{self, ClaimCursor},
    },
};
use devagotchi_store::Store;

const TENANT: &str = "t-00000000-0000-4000-8000-000000000001";
const OTHER: &str = "t-00000000-0000-4000-8000-000000000002";
const PRINCIPAL: &str = "@agent:alice:worker";

async fn board(store: &Store, tenant: &str, name: &str) -> HostedBoard {
    let board = HostedBoard::new(store.clone(), tenant, name).unwrap();
    board.initialize().await.unwrap();
    board
}

fn actor(id: &str) -> Actor<'_> {
    Actor {
        id,
        human: false,
        policy: None,
        wip_limit: Some(0),
    }
}

async fn claim(board: &HostedBoard, principal: &str, title: &str) -> crate::lease::Lease {
    let ticket = board
        .create(
            &Actor::human("owner"),
            &NewTicket {
                title: title.into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    board
        .claim(&actor(principal), &ticket.id, None)
        .await
        .unwrap()
}

async fn counts(store: &Store) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM devagotchi.board_events),
                          (SELECT count(*) FROM devagotchi.board_outbox)",
    )
    .fetch_one(store.admin())
    .await
    .unwrap()
}

#[tokio::test]
async fn totals_include_hidden_work_but_pages_reveal_only_authorized_live_claims() {
    let test = or_skip!(postgres());
    let a = board(&test.store, TENANT, "alpha").await;
    let b = board(&test.store, TENANT, "beta").await;
    let hidden = board(&test.store, TENANT, "hidden").await;
    let other = board(&test.store, OTHER, "alpha").await;
    let x = claim(&a, PRINCIPAL, "Visible A").await;
    let y = claim(&b, PRINCIPAL, "Visible B").await;
    claim(&hidden, PRINCIPAL, "Secret project task").await;
    claim(&other, PRINCIPAL, "Other tenant secret").await;
    claim(&a, "@agent:bob:worker", "Another principal secret").await;
    let expired = claim(&b, PRINCIPAL, "Expired work").await;
    sqlx::query(
        "UPDATE devagotchi.board_tickets SET lease_expires_at=0
                 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
    )
    .bind(TENANT)
    .bind(b.board_id())
    .bind(&expired.ticket.id)
    .execute(test.store.admin())
    .await
    .unwrap();
    let before = counts(&test.store).await;
    let visible = vec![a.board_id().into(), b.board_id().into()];
    let first = registry::principal_workload(&test.store, TENANT, PRINCIPAL, &visible, None, 1)
        .await
        .unwrap();
    assert_eq!(first.total, 3);
    assert_eq!(first.claims.len(), 1);
    assert_eq!(first.claims[0].cursor.board_id, a.board_id());
    assert_eq!(first.claims[0].cursor.ticket_id, x.ticket.id);
    assert_eq!(first.claims[0].title, "Visible A");
    assert!(first.claims[0].lease_expires_at > first.as_of);
    assert_eq!(first.next.as_ref(), Some(&first.claims[0].cursor));
    let second = registry::principal_workload(
        &test.store,
        TENANT,
        PRINCIPAL,
        &visible,
        first.next.as_ref(),
        1,
    )
    .await
    .unwrap();
    assert_eq!(second.total, 3);
    assert_eq!(second.claims.len(), 1);
    assert_eq!(second.claims[0].cursor.board_id, b.board_id());
    assert_eq!(second.claims[0].cursor.ticket_id, y.ticket.id);
    assert_eq!(second.claims[0].title, "Visible B");
    assert!(second.next.is_none());
    let empty = registry::principal_workload(&test.store, TENANT, PRINCIPAL, &[], None, 100)
        .await
        .unwrap();
    assert_eq!(empty.total, 3);
    assert!(empty.claims.is_empty() && empty.next.is_none());
    // A stale page cursor and a duplicate visible id confer no access to hidden tasks.
    let narrowed = registry::principal_workload(
        &test.store,
        TENANT,
        PRINCIPAL,
        &[a.board_id().into(), a.board_id().into()],
        first.next.as_ref(),
        100,
    )
    .await
    .unwrap();
    assert_eq!(narrowed.total, 3);
    assert!(narrowed.claims.is_empty());
    let missing =
        registry::principal_workload(&test.store, TENANT, "@agent:missing", &visible, None, 100)
            .await
            .unwrap();
    assert_eq!(missing.total, 0);
    assert!(missing.claims.is_empty());
    assert_eq!(
        counts(&test.store).await,
        before,
        "roster reads never emit events"
    );
}

#[tokio::test]
async fn workload_follows_release_completion_and_identical_ticket_ids_on_different_boards() {
    let test = or_skip!(postgres());
    let a = board(&test.store, TENANT, "alpha").await;
    let b = board(&test.store, TENANT, "beta").await;
    let x = claim(&a, PRINCIPAL, "A").await;
    let y = claim(&b, PRINCIPAL, "B").await;
    // Ticket identifiers are board-local. Keyset pagination must include the board.
    sqlx::query(
        "UPDATE devagotchi.board_tickets SET id=$3 WHERE tenant_id=$1 AND board_id=$2 AND id=$4",
    )
    .bind(TENANT)
    .bind(b.board_id())
    .bind(&x.ticket.id)
    .bind(&y.ticket.id)
    .execute(test.store.admin())
    .await
    .unwrap();
    let visible = vec![a.board_id().into(), b.board_id().into()];
    let first = registry::principal_workload(&test.store, TENANT, PRINCIPAL, &visible, None, 1)
        .await
        .unwrap();
    let second = registry::principal_workload(
        &test.store,
        TENANT,
        PRINCIPAL,
        &visible,
        first.next.as_ref(),
        1,
    )
    .await
    .unwrap();
    assert_eq!(first.total, 2);
    assert_eq!(second.claims.len(), 1);
    assert_eq!(
        first.claims[0].cursor.ticket_id,
        second.claims[0].cursor.ticket_id
    );
    assert_ne!(
        first.claims[0].cursor.board_id,
        second.claims[0].cursor.board_id
    );
    assert!(second.next.is_none());
    a.release(&actor(PRINCIPAL), &x.ticket.id, x.fence, "Capacity free")
        .await
        .unwrap();
    let remaining =
        registry::principal_workload(&test.store, TENANT, PRINCIPAL, &visible, None, 100)
            .await
            .unwrap();
    assert_eq!(remaining.total, 1);
    assert_eq!(remaining.claims.len(), 1);
    b.complete(&actor(PRINCIPAL), &x.ticket.id, y.fence, "Finished")
        .await
        .unwrap();
    let done = registry::principal_workload(&test.store, TENANT, PRINCIPAL, &visible, None, 100)
        .await
        .unwrap();
    assert_eq!(done.total, 0);
    assert!(done.claims.is_empty() && done.next.is_none());
}

#[tokio::test]
async fn invalid_page_bounds_refuse_and_unavailable_storage_never_reports_zero_work() {
    let test = or_skip!(postgres());
    for limit in [0, registry::MAX_WORKLOAD_CLAIMS + 1, usize::MAX] {
        assert_eq!(
            registry::principal_workload(&test.store, TENANT, PRINCIPAL, &[], None, limit)
                .await
                .unwrap_err()
                .code,
            "invalid"
        );
    }
    let too_many = vec!["alpha".into(); registry::MAX_WORKLOAD_BOARDS + 1];
    let too_long = "a".repeat(257);
    let bad_cursor = ClaimCursor {
        board_id: "alpha".into(),
        ticket_id: String::new(),
    };
    for (tenant, principal, visible, cursor) in [
        ("invalid", PRINCIPAL, &[][..], None),
        (TENANT, "", &[][..], None),
        (TENANT, too_long.as_str(), &[][..], None),
        (TENANT, PRINCIPAL, too_many.as_slice(), None),
        (TENANT, PRINCIPAL, &[][..], Some(&bad_cursor)),
    ] {
        assert_eq!(
            registry::principal_workload(&test.store, tenant, principal, visible, cursor, 1)
                .await
                .unwrap_err()
                .code,
            "invalid"
        );
    }
    // This standalone library fixture uses its database owner, not the host's role.
    // Make the read fail in this disposable database without relying on host roles.
    sqlx::query("ALTER TABLE devagotchi.board_tickets RENAME COLUMN holder TO unavailable_holder")
        .execute(test.store.admin())
        .await
        .unwrap();
    assert!(
        registry::principal_workload(&test.store, TENANT, PRINCIPAL, &[], None, 100)
            .await
            .is_err(),
        "a read failure must not become an idle agent"
    );
}
