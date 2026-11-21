//! Principal capacity across projects, with real simultaneous PostgreSQL transactions.
use super::{or_skip, postgres};
use crate::{Actor, NewTicket, NextFilter, hosted::HostedBoard};
use devagotchi_store::Store;
use std::sync::Arc;

const TENANT: &str = "t-00000000-0000-4000-8000-000000000001";
const OTHER: &str = "t-00000000-0000-4000-8000-000000000002";

async fn board(store: &Store, tenant: &str, owner: &str, name: &str) -> HostedBoard {
    let board = HostedBoard::for_owner(store.clone(), tenant, owner, name).unwrap();
    board.initialize().await.unwrap();
    board
}

async fn task(board: &HostedBoard, title: &str) -> String {
    board
        .create(
            &Actor::human("owner"),
            &NewTicket {
                title: title.into(),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
}

fn principal(id: &str, limit: i64) -> Actor<'_> {
    Actor {
        id,
        human: false,
        wip_limit: Some(limit),
    }
}

async fn counters(store: &Store, board: &HostedBoard) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT value FROM devagotchi.board_fence WHERE tenant_id=$1 AND board_id=$2),
                (SELECT count(*) FROM devagotchi.board_events WHERE tenant_id=$1 AND board_id=$2),
                (SELECT count(*) FROM devagotchi.board_outbox WHERE tenant_id=$1 AND board_id=$2)",
    )
    .bind(board.tenant())
    .bind(board.board_id())
    .fetch_one(store.admin())
    .await
    .unwrap()
}

#[tokio::test]
async fn principal_capacity_follows_live_work_across_projects_and_owner_namespaces() {
    let test = or_skip!(postgres());
    let a = board(&test.store, TENANT, "alice", "first").await;
    let b = board(&test.store, TENANT, "team", "second").await;
    // A host-authorized principal may work its own and shared boards with one identity.
    let worker = principal("alice/agent:worker-1", 1);
    let x = task(&a, "First").await;
    let y = task(&b, "Second").await;
    let held = a.claim(&worker, &x, None).await.unwrap();
    let before = counters(&test.store, &b).await;
    assert_eq!(
        b.claim(&worker, &y, None).await.unwrap_err().code,
        "wip_limit"
    );
    assert_eq!(
        b.claim_next(&worker, &NextFilter::default())
            .await
            .unwrap_err()
            .code,
        "wip_limit"
    );
    assert_eq!(counters(&test.store, &b).await, before);
    assert!(b.get(&y).await.unwrap().holder.is_none());
    let repeated = a.claim(&worker, &x, None).await.unwrap();
    assert!(repeated.already_held);
    assert_eq!(repeated.fence, held.fence);
    a.release(&worker, &x, held.fence, "Free capacity")
        .await
        .unwrap();
    let second = b
        .claim_next(&worker, &NextFilter::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.ticket.id, y);
    assert_eq!(
        a.claim(&worker, &x, None).await.unwrap_err().code,
        "wip_limit"
    );
    b.complete(&worker, &y, second.fence, "Ready for another task")
        .await
        .unwrap();
    let next = a.claim(&worker, &x, None).await.unwrap();
    assert!(next.fence > held.fence);
    let z = task(&b, "After expiry").await;
    sqlx::query(
        "UPDATE devagotchi.board_tickets SET lease_expires_at=0
         WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
    )
    .bind(TENANT)
    .bind(a.board_id())
    .bind(&x)
    .execute(test.store.admin())
    .await
    .unwrap();
    assert_eq!(b.claim(&worker, &z, None).await.unwrap().ticket.id, z);
    assert_eq!(
        a.heartbeat(&worker, &x, next.fence, None)
            .await
            .unwrap_err()
            .code,
        "lease_expired"
    );
}

#[tokio::test]
async fn principal_limits_keep_tenants_identities_zero_and_board_aliases_distinct() {
    let test = or_skip!(postgres());
    let a = board(&test.store, TENANT, "alice", "first").await;
    let b = board(&test.store, TENANT, "bob", "second").await;
    let other = board(&test.store, OTHER, "alice", "first").await;
    let worker = principal("alice/agent:worker-1", 1);
    let x = task(&a, "Principal in tenant A").await;
    a.claim(&worker, &x, None).await.unwrap();
    let y = task(&other, "Same principal id in tenant B").await;
    other.claim(&worker, &y, None).await.unwrap();
    let z = task(&b, "Different principal in tenant A").await;
    b.claim(&principal("bob/agent:worker-1", 1), &z, None)
        .await
        .unwrap();

    // Unregistered account-owned aliases remain board-local, not global identities.
    let alias = Actor::agent("claude");
    for board in [&a, &b] {
        let mut settings = board.settings().await.unwrap();
        settings.wip_limit_default = 1;
        board
            .put_settings(&Actor::human("owner"), &settings)
            .await
            .unwrap();
        let first = task(board, "Alias first").await;
        let second = task(board, "Alias second").await;
        board.claim(&alias, &first, None).await.unwrap();
        assert_eq!(
            board.claim(&alias, &second, None).await.unwrap_err().code,
            "wip_limit"
        );
        // A deliberate zero principal limit overrides the board default as before.
        board
            .claim(&principal("alice/agent:unlimited", 0), &second, None)
            .await
            .unwrap();
        let third = task(board, "Unlimited third").await;
        board
            .claim(&principal("alice/agent:unlimited", 0), &third, None)
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_claims_on_six_boards_share_one_principal_capacity() {
    let test = or_skip!(postgres());
    let start = Arc::new(tokio::sync::Barrier::new(6));
    let mut tasks = Vec::new();
    let mut boards = Vec::new();
    for i in 0..6 {
        let board = board(&test.store, TENANT, "alice", &format!("project-{i}")).await;
        let id = task(&board, "Race for global capacity").await;
        let before = counters(&test.store, &board).await;
        let peer = board.clone();
        let start = start.clone();
        let submitted = id.clone();
        tasks.push(tokio::spawn(async move {
            start.wait().await;
            let worker = principal("alice/agent:worker-1", 1);
            if i % 2 == 0 {
                peer.claim(&worker, &submitted, None).await
            } else {
                peer.claim_next(&worker, &NextFilter::default())
                    .await
                    .map(|lease| lease.expect("board has one ready task"))
            }
        }));
        boards.push((board, id, before));
    }
    let mut winners = 0;
    for (task, (board, id, before)) in tasks.into_iter().zip(boards) {
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), task)
            .await
            .expect("cross-board claims must settle")
            .unwrap();
        match result {
            Ok(lease) => {
                winners += 1;
                assert_eq!(lease.ticket.id, id);
                assert_eq!(
                    board.get(&id).await.unwrap().holder.as_deref(),
                    Some("alice/agent:worker-1")
                );
                assert_eq!(
                    counters(&test.store, &board).await,
                    (before.0 + 1, before.1 + 1, before.2 + 1)
                );
            }
            Err(error) => {
                assert_eq!(error.code, "wip_limit", "{error}");
                assert!(board.get(&id).await.unwrap().holder.is_none());
                assert_eq!(counters(&test.store, &board).await, before);
            }
        }
    }
    assert_eq!(
        winners, 1,
        "independent board locks cannot admit extra work"
    );
}
