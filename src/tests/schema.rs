//! The change signal: a committed event is announced on Postgres's channel, a write that
//! records nothing is not, and a reported change wakes whoever waits on the board.

use std::time::Duration;

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_committed_change_is_announced_and_an_empty_reap_is_not() {
    let f = or_skip!(fixture());
    let mut listener = super::board_changes(&f.test.url).await;
    let id = f.new_ticket(&Actor::human("jesse"), "wake the waiters");
    let heard = tokio::time::timeout(Duration::from_secs(10), listener.recv())
        .await
        .expect("a committed event wakes waiters")
        .unwrap();
    assert_eq!(
        heard.payload(),
        serde_json::json!([LOCAL_TENANT, PROJECT]).to_string(),
        "named by its tenant and board"
    );
    assert_eq!(f.board.reap().unwrap(), 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), listener.recv())
            .await
            .is_err(),
        "a write that records nothing wakes nobody"
    );
    assert_eq!(f.board.next_lease_expiry().unwrap(), None);
    let lease = f
        .board
        .claim(&Actor::agent("claude"), &id, Some(30_000))
        .unwrap();
    assert_eq!(
        lease.lease_expires_at,
        lease.ticket.heartbeat_at.unwrap() + 30_000
    );
    assert_eq!(
        f.board.next_lease_expiry().unwrap(),
        Some(lease.lease_expires_at)
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(10), listener.recv())
            .await
            .is_ok(),
        "the claim is announced"
    );

    // What the hub's listener does with an announcement: wake the board's waiters.
    let changes = f.board.changes();
    assert!(!changes.has_changed().unwrap());
    f.board.mark_changed();
    assert!(changes.has_changed().unwrap());
}
