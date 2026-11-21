use super::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_replay_keeps_original_transition_and_rechecks_its_current_policy() {
    use crate::policy::{Effect, Identity, Rule, Selector, Subject, catalog};
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let id = f.new_ticket(&human, "Update");
    let worker = Actor {
        policy: Some(Identity {
            kind: "codex".into(),
            roles: vec!["worker".into()],
        }),
        ..Actor::agent("worker")
    };
    let roles = vec!["admin".into()];
    let owner = Subject {
        id: "owner",
        human: true,
        kind: "human",
        roles: &roles,
    };
    transaction!(&f.test.store, |connection| {
        f.test.store.scope(connection, LOCAL_TENANT).await?;
        catalog::initialize_in(connection, LOCAL_TENANT).await?;
        Ok(())
    })
    .unwrap();
    let input = json!({"id":id,"version":1,"state":"blocked"});
    let board = f
        .board
        .requested("worker", "edit-one", "update_task", &input)
        .unwrap();
    let patch = Patch {
        state: Some("blocked".into()),
        ..Default::default()
    };
    let changed = board.update(&worker, &id, 1, patch.clone()).unwrap();
    assert_eq!(changed.version, 2);
    assert_eq!(
        board.update(&worker, &id, 1, patch.clone()).unwrap(),
        changed
    );
    assert!(board.hosted().replayed());
    assert_eq!(
        f.board
            .update(&worker, &id, 1, patch.clone())
            .unwrap_err()
            .code,
        "version_conflict"
    );
    transaction!(&f.test.store, |connection| {
        f.test.store.scope(connection, LOCAL_TENANT).await?;
        catalog::put_rule_in(
            connection,
            LOCAL_TENANT,
            &owner,
            &Rule {
                id: "deny-original-transition".into(),
                name: "Withdraw transition".into(),
                subject: "role:worker".into(),
                action: "task.transition:ready->blocked".into(),
                selector: Selector::default(),
                effect: Effect::Deny,
                priority: 1000,
                enabled: true,
                version: 0,
            },
        )
        .await?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        board.update(&worker, &id, 1, patch).unwrap_err().code,
        "policy_denied"
    );
    assert_eq!(f.board.events(0, 100).unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_create_replays_original_ticket_once_with_one_event_and_outbox_row() {
    let f = or_skip!(fixture());
    let actor = Actor::human("owner");
    let input = json!({"title":"One task"});
    let one = f
        .board
        .requested("owner", "create-one", "create_task", &input)
        .unwrap();
    let reopened = crate::open_board(&f.test.url, PROJECT, false)
        .await
        .unwrap();
    let two = reopened
        .requested("owner", "create-one", "create_task", &input)
        .unwrap();
    let new = NewTicket {
        title: "One task".into(),
        ..Default::default()
    };
    let (first, second) = tokio::join!(
        one.hosted().create(&actor, &new),
        two.hosted().create(&actor, &new)
    );
    let first = first.unwrap();
    assert_eq!(first, second.unwrap());
    assert_ne!(one.hosted().replayed(), two.hosted().replayed());
    let events = f.board.events(0, 100).unwrap();
    assert_eq!(events.len(), 1);
    let outbox: i64 = sqlx::query_scalar("SELECT count(*) FROM board_outbox")
        .fetch_one(f.test.store.admin())
        .await
        .unwrap();
    assert_eq!(outbox, 1);
    f.board
        .update(
            &actor,
            &first.id,
            first.version,
            Patch {
                title: Some("Changed later".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        one.create(&actor, new.clone()).unwrap(),
        first,
        "receipt is the original response, not a fresh read"
    );
    assert!(one.hosted().replayed());
    let conflict = f
        .board
        .requested(
            "owner",
            "create-one",
            "create_task",
            &json!({"title":"Different"}),
        )
        .unwrap();
    assert_eq!(
        conflict
            .create(
                &actor,
                NewTicket {
                    title: "Different".into(),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
        "request_conflict"
    );
    let other = f
        .board
        .requested("another", "create-one", "create_task", &input)
        .unwrap();
    assert_ne!(other.create(&actor, new).unwrap().id, first.id);
    assert_eq!(f.board.events(0, 100).unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn note_and_dependency_retries_replay_once_and_current_policy_can_refuse_them() {
    use crate::policy::{Effect, Identity, Rule, Selector, Subject, catalog};
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let id = f.new_ticket(&human, "Work");
    let target = f.new_ticket(&human, "Prerequisite");
    let worker = Actor {
        policy: Some(Identity {
            kind: "codex".into(),
            roles: vec!["worker".into()],
        }),
        ..Actor::agent("worker")
    };
    let roles = vec!["admin".into()];
    let owner = Subject {
        id: "owner",
        human: true,
        kind: "human",
        roles: &roles,
    };
    transaction!(&f.test.store, |connection| {
        f.test.store.scope(connection, LOCAL_TENANT).await?;
        catalog::initialize_in(connection, LOCAL_TENANT).await?;
        Ok(())
    })
    .unwrap();
    let note_input = json!({"id":id,"body":"Evidence"});
    let note_board = f
        .board
        .requested("worker", "note-one", "add_note", &note_input)
        .unwrap();
    let before = f.board.events(0, 100).unwrap().len();
    let note = note_board
        .add_note(&worker, &id, "note", "Evidence", None, None)
        .unwrap();
    assert_eq!(
        note_board
            .add_note(&worker, &id, "note", "Evidence", None, None)
            .unwrap(),
        note
    );
    assert!(note_board.hosted().replayed());
    assert_eq!(f.board.events(0, 100).unwrap().len(), before + 1);
    let dep_input = json!({"id":id,"depends_on":target,"kind":"blocks"});
    let deps = f
        .board
        .requested("worker", "dep-one", "add_dep", &dep_input)
        .unwrap();
    let original = deps
        .add_dep(&worker, &id, &target, DepKind::Blocks)
        .unwrap();
    f.board
        .remove_dep(&human, &id, &target, DepKind::Blocks)
        .unwrap();
    assert_eq!(
        deps.add_dep(&worker, &id, &target, DepKind::Blocks)
            .unwrap(),
        original
    );
    assert!(deps.hosted().replayed());
    assert!(
        f.board.get(&id).unwrap().deps.is_empty(),
        "replay cannot recreate a removed dependency"
    );
    transaction!(&f.test.store, |connection| {
        f.test.store.scope(connection, LOCAL_TENANT).await?;
        catalog::put_rule_in(
            connection,
            LOCAL_TENANT,
            &owner,
            &Rule {
                id: "deny-replay".into(),
                name: "Withdraw worker edits".into(),
                subject: "role:worker".into(),
                action: "*".into(),
                selector: Selector::default(),
                effect: Effect::Deny,
                priority: 1000,
                enabled: true,
                version: 0,
            },
        )
        .await?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        note_board
            .add_note(&worker, &id, "note", "Evidence", None, None)
            .unwrap_err()
            .code,
        "policy_denied"
    );
    assert_eq!(
        deps.add_dep(&worker, &id, &target, DepKind::Blocks)
            .unwrap_err()
            .code,
        "policy_denied"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_mutation_does_not_consume_key_and_operation_identity_is_shared() {
    let f = or_skip!(fixture());
    let actor = Actor::human("owner");
    let target = f.new_ticket(&actor, "Target");
    let input = json!({"title":"Child","depends_on":[target]});
    let requested = f
        .board
        .requested("owner", "retry", "create_task", &input)
        .unwrap();
    let mut new = NewTicket {
        title: "Child".into(),
        depends_on: vec!["missing".into()],
        ..Default::default()
    };
    assert_eq!(
        requested.create(&actor, new.clone()).unwrap_err().code,
        "not_found"
    );
    new.depends_on = vec![target];
    let made = requested.create(&actor, new).unwrap();
    assert!(!requested.hosted().replayed());
    let note = f
        .board
        .requested(
            "owner",
            "retry",
            "add_note",
            &json!({"id":made.id,"body":"Note"}),
        )
        .unwrap();
    assert_eq!(
        note.add_note(&actor, &made.id, "note", "Note", None, None)
            .unwrap_err()
            .code,
        "request_conflict"
    );
    assert_eq!(f.board.events(0, 100).unwrap().len(), 2);
}
