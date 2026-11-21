use super::*;
use crate::hosted::{HostedBoard, PrincipalDirectory, TaskLeaseBinding};
use devagotchi_store::{Failed, Store};
use sqlx::AnyConnection;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct Directory(Arc<AtomicBool>);
impl PrincipalDirectory for Directory {
    fn authorize_request<'a>(
        &'a self,
        _c: &'a mut AnyConnection,
        _tenant: &'a str,
        principal: &'a str,
        actor: &'a Actor<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Failed>> + Send + 'a>> {
        Box::pin(async move {
            if !self.0.load(Ordering::SeqCst) || principal != "worker" || actor.id != principal {
                return Err(crate::fail("not_granted", "principal disabled or changed").into());
            }
            Ok(())
        })
    }
    fn enabled<'a>(
        &'a self,
        _c: &'a mut AnyConnection,
        _tenant: &'a str,
        principal: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, Failed>> + Send + 'a>> {
        Box::pin(async move { Ok(principal == "worker" && self.0.load(Ordering::SeqCst)) })
    }
}
const TENANT: &str = "t-00000000-0000-4000-8000-000000000001";
async fn fixture(store: &Store) -> (HostedBoard, crate::Lease, Arc<AtomicBool>) {
    let enabled = Arc::new(AtomicBool::new(true));
    let board = HostedBoard::new(store.clone(), TENANT, "access")
        .unwrap()
        .with_principals(Arc::new(Directory(enabled.clone())));
    board.initialize().await.unwrap();
    let task = board
        .create(
            &Actor::human("owner"),
            &NewTicket {
                title: "Machine work".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let lease = board
        .claim(&Actor::agent("worker"), &task.id, None)
        .await
        .unwrap();
    (board, lease, enabled)
}
async fn bind(
    store: &Store,
    board: &HostedBoard,
    lease: &crate::Lease,
) -> devagotchi_contracts::BridgeResult<TaskLeaseBinding> {
    transaction!(store, |c| {
        board
            .bind_access_task_in(
                c,
                "worker",
                &Actor::agent("worker"),
                &lease.ticket.id,
                lease.fence,
            )
            .await
    })
}
async fn check(
    store: &Store,
    board: &HostedBoard,
    binding: &TaskLeaseBinding,
) -> devagotchi_contracts::BridgeResult<TaskLeaseBinding> {
    transaction!(store, |c| {
        board
            .check_access_task_in(c, "worker", &Actor::agent("worker"), binding)
            .await
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn access_binding_tracks_heartbeat_but_not_a_new_lease() {
    let test = or_skip!(super::postgres());
    let (board, lease, _) = fixture(&test.store).await;
    let original = bind(&test.store, &board, &lease).await.unwrap();
    assert_eq!(original.lease_id, lease.lease_id);
    assert_eq!(original.expires_at, lease.lease_expires_at);
    let beat = board
        .heartbeat(&Actor::agent("worker"), &lease.ticket.id, lease.fence, None)
        .await
        .unwrap();
    let current = check(&test.store, &board, &original).await.unwrap();
    assert_eq!(current.expires_at, beat.lease_expires_at);
    assert_eq!(current.lease_id, original.lease_id);
    let mut forged = original.clone();
    forged.lease_id = "another-lease".into();
    assert_eq!(
        check(&test.store, &board, &forged).await.unwrap_err().code,
        "stale_fence"
    );
    board
        .release(
            &Actor::agent("worker"),
            &lease.ticket.id,
            lease.fence,
            "pause",
        )
        .await
        .unwrap();
    assert!(check(&test.store, &board, &original).await.is_err());
    let next = board
        .claim(&Actor::agent("worker"), &lease.ticket.id, None)
        .await
        .unwrap();
    assert!(next.fence > original.fence);
    assert_eq!(
        check(&test.store, &board, &original)
            .await
            .unwrap_err()
            .code,
        "stale_fence"
    );
    bind(&test.store, &board, &next).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn access_binding_requires_current_identity_generation_and_database_expiry() {
    let test = or_skip!(super::postgres());
    let (board, lease, enabled) = fixture(&test.store).await;
    let original = bind(&test.store, &board, &lease).await.unwrap();
    enabled.store(false, Ordering::SeqCst);
    assert_eq!(
        check(&test.store, &board, &original)
            .await
            .unwrap_err()
            .code,
        "not_granted"
    );
    enabled.store(true, Ordering::SeqCst);
    let bare = HostedBoard::new(test.store.clone(), TENANT, "access").unwrap();
    assert_eq!(
        bind(&test.store, &bare, &lease).await.unwrap_err().code,
        "not_granted"
    );
    for field in ["tenant", "board", "principal"] {
        let mut altered = original.clone();
        match field {
            "tenant" => altered.tenant.push('x'),
            "board" => altered.board.push('x'),
            _ => altered.principal = "other".into(),
        }
        assert_eq!(
            check(&test.store, &board, &altered).await.unwrap_err().code,
            "not_granted"
        );
    }
    let result = transaction!(&test.store, |c| {
        board
            .bind_access_task_in(
                c,
                "worker",
                &Actor::human("owner"),
                &lease.ticket.id,
                lease.fence,
            )
            .await
    });
    assert_eq!(result.unwrap_err().code, "not_granted");
    transaction!(&test.store, |c| {
        test.store.scope(c, TENANT).await?;
        sqlx::query("UPDATE board_tickets SET lease_expires_at=1 WHERE tenant_id=$1 AND board_id=$2 AND id=$3")
            .bind(TENANT).bind(board.board_id()).bind(&lease.ticket.id).execute(c).await?;
        Ok(())
    }).unwrap();
    assert_eq!(
        check(&test.store, &board, &original)
            .await
            .unwrap_err()
            .code,
        "lease_expired"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn access_binding_holds_the_board_lock_until_the_access_transaction_finishes() {
    let test = or_skip!(super::postgres());
    let (board, lease, _) = fixture(&test.store).await;
    let mut c = test.store.pool().begin().await.unwrap();
    let binding = board
        .bind_access_task_in(
            &mut c,
            "worker",
            &Actor::agent("worker"),
            &lease.ticket.id,
            lease.fence,
        )
        .await
        .unwrap_or_else(|error| match error {
            Failed::Store(error) => panic!("binding storage failed: {error}"),
            Failed::Refused(error) => panic!("binding refused: {}: {}", error.code, error.message),
        });
    let competing = board.clone();
    let id = lease.ticket.id.clone();
    let mut release = tokio::spawn(async move {
        competing
            .release(&Actor::agent("worker"), &id, lease.fence, "finished")
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(150), &mut release)
            .await
            .is_err()
    );
    c.commit().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), release)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(check(&test.store, &board, &binding).await.is_err());
}
