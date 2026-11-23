use super::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_keys_follow_events_and_outbox_without_replaying_or_leaking_into_direct_writes() {
    let f = or_skip!(fixture());
    let actor = Actor::human("owner");
    let create = f
        .board
        .requested(
            "owner",
            "create-1",
            "create_task",
            &json!({"title":"Trace this"}),
        )
        .unwrap();
    let ticket = create
        .create(
            &actor,
            NewTicket {
                title: "Trace this".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let delegate = f
        .board
        .requested(
            "owner",
            "delegate-1",
            "delegate",
            &json!({"id":ticket.id,"version":1,"to":"codex","note":"Review"}),
        )
        .unwrap();
    let saved = delegate
        .delegate(&actor, &ticket.id, 1, "codex", "Review")
        .unwrap();
    let events = f.board.events(0, 100).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].request_id.as_deref(), Some("create-1"));
    assert!(
        events[1..]
            .iter()
            .all(|event| event.request_id.as_deref() == Some("delegate-1"))
    );
    assert_eq!(
        delegate
            .delegate(&actor, &ticket.id, 1, "codex", "Review")
            .unwrap(),
        saved
    );
    assert_eq!(f.board.events(0, 100).unwrap(), events);
    let payloads: Vec<String> =
        sqlx::query_scalar("SELECT payload FROM board_outbox ORDER BY event_id")
            .fetch_all(f.test.store.admin())
            .await
            .unwrap();
    let outbox: Vec<crate::Event> = payloads
        .iter()
        .map(|payload| serde_json::from_str(payload).unwrap())
        .collect();
    assert_eq!(outbox, events);
    for request_id in [None, Some("changed".to_string())] {
        let mut tampered = events[0].clone();
        tampered.request_id = request_id;
        assert_ne!(
            crate::event_hash(&tampered.prev_hash, &tampered),
            events[0].hash
        );
    }
    f.new_ticket(&actor, "Direct operation");
    let direct = f.board.events(0, 100).unwrap().pop().unwrap();
    assert!(direct.request_id.is_none());
    assert!(
        serde_json::to_value(&direct)
            .unwrap()
            .get("request_id")
            .is_none()
    );
    assert!(f.board.hosted().verify_chain().await.unwrap().ok);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_request_correlates_both_events_of_an_approval_and_standing_grant() {
    let f = or_skip!(fixture());
    let (asked, _) = f
        .board
        .ask(
            &Actor::agent("worker"),
            &crate::approval::Ask {
                title: "Restart service",
                what: "systemctl restart example",
                why: "Apply configuration",
                undo: "Restore configuration",
                kind: Some("restart"),
                target: Some("example"),
            },
        )
        .unwrap();
    let requested = f
        .board
        .requested(
            "owner",
            "approve-and-allow",
            "decide_and_allow",
            &json!({"id":asked.id,"version":asked.version,"days":1}),
        )
        .unwrap();
    let (_, grant) = requested
        .decide_and_allow(
            &Actor::human("owner"),
            &asked.id,
            asked.version,
            "Approved",
            1,
        )
        .unwrap();
    assert!(grant.is_some());
    let events = f.board.events(0, 100).unwrap();
    assert_eq!(events.len(), 3);
    assert!(events[0].request_id.is_none());
    assert_eq!(events[1].action, crate::chain::actions::APPROVAL_DECIDED);
    assert_eq!(
        events[2].action,
        crate::chain::actions::APPROVAL_STANDING_GRANTED
    );
    assert!(
        events[1..]
            .iter()
            .all(|event| event.request_id.as_deref() == Some("approve-and-allow"))
    );
    assert!(f.board.hosted().verify_chain().await.unwrap().ok);
}

#[tokio::test]
async fn request_correlation_migration_preserves_the_existing_event_and_hash() {
    use crate::{GENESIS, hosted::HostedBoard, migrations::MIGRATIONS};
    use devagotchi_store::testing;
    let url = or_skip!(testing::empty_postgres());
    testing::migrated_to(&url, &MIGRATIONS, 8).await;
    let mut connection = <sqlx::AnyConnection as sqlx::Connection>::connect(&url)
        .await
        .unwrap();
    sqlx::query("INSERT INTO devagotchi.board_projects (tenant_id,id,source_hash,imported_at) VALUES ($1,'existing','native',1)")
        .bind(LOCAL_TENANT).execute(&mut connection).await.unwrap();
    // Fixed pre-migration canonical hash: adding a nullable column must not rewrite it.
    let hash = "d9535d05b8c2cbe454412f332ae3a432610820906f69dc5a65b381e3b83e31f5";
    sqlx::query("INSERT INTO devagotchi.board_events (tenant_id,board_id,seq,at,actor,action,ticket,data,fence,prev_hash,hash) VALUES ($1,'existing',1,1234,'owner','ticket.created',NULL,$2,NULL,$3,$4)")
        .bind(LOCAL_TENANT).bind(json!({"title":"Before request correlation"}).to_string()).bind(GENESIS).bind(hash)
        .execute(&mut connection).await.unwrap();
    let store = devagotchi_store::Store::open(&url, &[&MIGRATIONS])
        .await
        .unwrap();
    let board = HostedBoard::new(store, LOCAL_TENANT, "existing").unwrap();
    let events = board.events().await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].hash, hash);
    assert!(events[0].request_id.is_none());
    let verified = board.verify_chain().await.unwrap();
    assert!(verified.ok, "{verified:?}");
    assert_eq!(verified.head, hash);
}
