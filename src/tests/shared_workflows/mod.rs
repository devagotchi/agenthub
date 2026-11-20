//! Real PostgreSQL sharing, races, isolation and rollback across boards.
use super::{or_skip, postgres};
use crate::{Actor, NewTicket, StateKind, actions, hosted::HostedBoard, workflow::State};

const TENANT: &str = "t-00000000-0000-4000-8000-000000000001";
const OTHER: &str = "t-00000000-0000-4000-8000-000000000002";
mod rls;

async fn board(
    store: &devagotchi_store::Store,
    tenant: &str,
    owner: &str,
    name: &str,
) -> HostedBoard {
    let board = HostedBoard::for_owner(store.clone(), tenant, owner, name).unwrap();
    board.initialize().await.unwrap();
    board
}

#[tokio::test]
async fn shared_definition_changes_real_work_on_both_boards_and_detaches_without_loss() {
    let test = or_skip!(postgres());
    let a = board(&test.store, TENANT, "alice", "first").await;
    let b = board(&test.store, TENANT, "alice", "second").await;
    let human = Actor::human("alice");
    let worker = Actor::agent("worker");
    let published = a
        .publish_workflow(&human, 1, "delivery", "Delivery")
        .await
        .unwrap();
    assert_eq!(published.version, 1);
    assert_eq!(published.used_by, ["first"]);
    assert_eq!(
        b.publish_workflow(&human, 1, "delivery", "Duplicate")
            .await
            .unwrap_err()
            .code,
        "already_exists"
    );
    assert!(b.events().await.unwrap().is_empty());
    assert_eq!(b.configuration_state().await.unwrap().workflow_version, 1);
    b.bind_workflow(&human, 1, "delivery", 1).await.unwrap();
    let before = a.workflow_configuration().await.unwrap();
    assert_eq!(before.shared_workflow.unwrap().id, "delivery");
    assert_eq!(before.configuration.workflow_version, 2);
    let events = b.events().await.unwrap();
    b.bind_workflow(&human, 2, "delivery", 1).await.unwrap();
    assert_eq!(b.events().await.unwrap(), events, "same binding is a no-op");
    let task = b
        .create(
            &human,
            &NewTicket {
                title: "Already held".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let held = b.claim(&worker, &task.id, None).await.unwrap();
    let mut workflow = published.workflow.clone();
    let done = workflow
        .states
        .iter()
        .find(|s| s.kind == StateKind::Done)
        .unwrap()
        .key
        .clone();
    workflow.name = "Direct completion".into();
    workflow.lanes[0].complete_to = done.clone();
    let edited = a
        .edit_shared_workflow(&human, "delivery", 1, &workflow)
        .await
        .unwrap();
    assert_eq!(edited.version, 2);
    assert_eq!(edited.used_by, ["first", "second"]);
    for board in [&a, &b] {
        let now = board.configuration_state().await.unwrap();
        assert_eq!(now.workflow, workflow);
        assert_eq!(now.workflow_version, 3);
        assert_eq!(
            board
                .edit_workflow(&human, 3, &workflow)
                .await
                .unwrap_err()
                .code,
            "shared_workflow"
        );
        assert_eq!(
            board
                .put_workflow(&human, &workflow)
                .await
                .unwrap_err()
                .code,
            "shared_workflow"
        );
    }
    assert_eq!(
        b.complete(
            &worker,
            &task.id,
            held.fence,
            "Complete under the shared rules"
        )
        .await
        .unwrap()
        .state,
        done
    );
    let a_events = a.events().await.unwrap();
    let b_events = b.events().await.unwrap();
    a.edit_shared_workflow(&human, "delivery", 2, &workflow)
        .await
        .unwrap();
    assert_eq!(a.events().await.unwrap(), a_events);
    assert_eq!(b.events().await.unwrap(), b_events);
    assert_eq!(
        a.delete_shared_workflow(&human, "delivery", 2)
            .await
            .unwrap_err()
            .code,
        "in_use"
    );
    assert_eq!(
        b.detach_workflow(&human, 2).await.unwrap_err().code,
        "version_conflict"
    );
    let detached = b.detach_workflow(&human, 3).await.unwrap();
    assert_eq!(detached.workflow, workflow);
    assert_eq!(detached.workflow_version, 4);
    assert!(
        b.workflow_configuration()
            .await
            .unwrap()
            .shared_workflow
            .is_none()
    );
    let mut next = workflow.clone();
    next.name = "Only the first board".into();
    a.edit_shared_workflow(&human, "delivery", 2, &next)
        .await
        .unwrap();
    assert_eq!(b.workflow().await.unwrap(), workflow);
    a.detach_workflow(&human, 4).await.unwrap();
    a.delete_shared_workflow(&human, "delivery", 3)
        .await
        .unwrap();
    assert!(a.shared_workflows().await.unwrap().is_empty());
    assert_eq!(a.workflow().await.unwrap(), next);
    assert!(
        a.events()
            .await
            .unwrap()
            .iter()
            .any(|e| e.action == actions::WORKFLOW_DELETED)
    );
    a.verify_chain().await.unwrap();
    b.verify_chain().await.unwrap();
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM devagotchi.board_events),(SELECT count(*) FROM devagotchi.board_outbox)")
        .fetch_one(test.store.admin()).await.unwrap();
    assert_eq!(
        counts.0, counts.1,
        "every committed event reaches the outbox"
    );
}

#[tokio::test]
async fn a_used_state_on_one_board_refuses_the_whole_shared_edit() {
    let test = or_skip!(postgres());
    let a = board(&test.store, TENANT, "alice", "first").await;
    let b = board(&test.store, TENANT, "alice", "second").await;
    let human = Actor::human("alice");
    let mut original = a.workflow().await.unwrap();
    original.states.push(State {
        key: "planned".into(),
        label: "Planned".into(),
        kind: StateKind::Queue,
    });
    original.initial_state = "planned".into();
    a.edit_workflow(&human, 1, &original).await.unwrap();
    let published = a
        .publish_workflow(&human, 2, "delivery", "Delivery")
        .await
        .unwrap();
    b.bind_workflow(&human, 1, "delivery", 1).await.unwrap();
    b.create(
        &human,
        &NewTicket {
            title: "Keep my state".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let first = a.configuration_state().await.unwrap();
    let second = b.configuration_state().await.unwrap();
    let events_a = a.events().await.unwrap();
    let events_b = b.events().await.unwrap();
    let mut invalid = published.workflow;
    invalid.states.retain(|s| s.key != "planned");
    invalid.initial_state = "ready".into();
    invalid.validate().unwrap();
    assert!(
        a.edit_shared_workflow(&human, "delivery", 1, &invalid)
            .await
            .is_err()
    );
    assert_eq!(a.configuration_state().await.unwrap(), first);
    assert_eq!(b.configuration_state().await.unwrap(), second);
    assert_eq!(a.events().await.unwrap(), events_a);
    assert_eq!(b.events().await.unwrap(), events_b);
    assert_eq!(a.shared_workflows().await.unwrap()[0].version, 1);
    // Rebinding cannot strand the same task either.
    a.detach_workflow(&human, first.workflow_version)
        .await
        .unwrap();
    let current = a.configuration_state().await.unwrap();
    a.edit_workflow(&human, current.workflow_version, &invalid)
        .await
        .unwrap();
    let current = a.configuration_state().await.unwrap();
    a.publish_workflow(&human, current.workflow_version, "other", "Other")
        .await
        .unwrap();
    assert!(
        b.bind_workflow(&human, second.workflow_version, "other", 1)
            .await
            .is_err()
    );
    assert_eq!(
        b.workflow_configuration()
            .await
            .unwrap()
            .shared_workflow
            .unwrap()
            .id,
        "delivery"
    );
    a.verify_chain().await.unwrap();
    b.verify_chain().await.unwrap();
}

#[tokio::test]
async fn catalog_is_owner_and_tenant_scoped_and_only_humans_change_it() {
    let test = or_skip!(postgres());
    let a = board(&test.store, TENANT, "alice", "demo").await;
    let other_owner = board(&test.store, TENANT, "bob", "demo").await;
    let other_tenant = board(&test.store, OTHER, "alice", "demo").await;
    let human = Actor::human("alice");
    let agent = Actor::agent("agent");
    let published = a
        .publish_workflow(&human, 1, "delivery", "Private")
        .await
        .unwrap();
    for outsider in [&other_owner, &other_tenant] {
        assert!(outsider.shared_workflows().await.unwrap().is_empty());
        assert_eq!(
            outsider
                .bind_workflow(&human, 1, "delivery", 1)
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        assert_eq!(
            outsider
                .edit_shared_workflow(&human, "delivery", 1, &published.workflow)
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        assert_eq!(
            outsider
                .delete_shared_workflow(&human, "delivery", 1)
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        outsider
            .publish_workflow(&human, 1, "delivery", "Independent")
            .await
            .unwrap();
    }
    let before = a.events().await.unwrap();
    assert_eq!(
        a.publish_workflow(&agent, 2, "another", "No")
            .await
            .unwrap_err()
            .code,
        "humans_only"
    );
    assert_eq!(
        a.bind_workflow(&agent, 2, "delivery", 1)
            .await
            .unwrap_err()
            .code,
        "humans_only"
    );
    assert_eq!(
        a.detach_workflow(&agent, 2).await.unwrap_err().code,
        "humans_only"
    );
    assert_eq!(
        a.edit_shared_workflow(&agent, "delivery", 1, &published.workflow)
            .await
            .unwrap_err()
            .code,
        "humans_only"
    );
    assert_eq!(
        a.delete_shared_workflow(&agent, "delivery", 1)
            .await
            .unwrap_err()
            .code,
        "humans_only"
    );
    assert_eq!(a.events().await.unwrap(), before);
    assert_eq!(a.workflow().await.unwrap().name, "Private");
    let tables: Vec<(String, bool, bool)> = sqlx::query_as("SELECT relname::text,relrowsecurity,relforcerowsecurity FROM pg_class WHERE relname IN ('board_workflows','board_workflow_bindings')")
        .fetch_all(test.store.admin()).await.unwrap();
    assert_eq!(tables.len(), 2);
    assert!(
        tables
            .iter()
            .all(|(_, enabled, forced)| *enabled && *forced)
    );
    rls::prove(&test.store).await;
}

#[tokio::test]
async fn concurrent_catalog_edits_and_bindings_have_a_consistent_versioned_winner() {
    let test = or_skip!(postgres());
    let a = board(&test.store, TENANT, "alice", "first").await;
    let b = board(&test.store, TENANT, "alice", "second").await;
    let human = Actor::human("alice");
    let published = a
        .publish_workflow(&human, 1, "delivery", "Delivery")
        .await
        .unwrap();
    let mut first = published.workflow.clone();
    first.name = "First".into();
    let mut second = published.workflow;
    second.name = "Second".into();
    let (x, y) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            a.edit_shared_workflow(&human, "delivery", 1, &first),
            b.edit_shared_workflow(&human, "delivery", 1, &second)
        )
    })
    .await
    .expect("catalog lock cannot deadlock against another board");
    assert_ne!(x.is_ok(), y.is_ok());
    assert_eq!(x.err().or(y.err()).unwrap().code, "version_conflict");
    assert_eq!(
        b.bind_workflow(&human, 1, "delivery", 1)
            .await
            .unwrap_err()
            .code,
        "version_conflict"
    );
    let mut latest = a.workflow().await.unwrap();
    latest.name = "Third".into();
    let (edit, bind) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            a.edit_shared_workflow(&human, "delivery", 2, &latest),
            b.bind_workflow(&human, 1, "delivery", 2)
        )
    })
    .await
    .expect("binding and edit serialize");
    edit.unwrap();
    if bind.is_ok() {
        assert_eq!(b.workflow().await.unwrap(), latest);
        assert_eq!(b.configuration_state().await.unwrap().workflow_version, 3);
    } else {
        assert_eq!(bind.unwrap_err().code, "version_conflict");
        assert!(
            b.workflow_configuration()
                .await
                .unwrap()
                .shared_workflow
                .is_none()
        );
    }
    a.verify_chain().await.unwrap();
    b.verify_chain().await.unwrap();
}

#[tokio::test]
async fn storage_failure_rolls_back_definition_versions_events_and_bindings() {
    let test = or_skip!(postgres());
    let a = board(&test.store, TENANT, "alice", "first").await;
    let b = board(&test.store, TENANT, "alice", "second").await;
    let human = Actor::human("alice");
    let published = a
        .publish_workflow(&human, 1, "delivery", "Delivery")
        .await
        .unwrap();
    b.bind_workflow(&human, 1, "delivery", 1).await.unwrap();
    let first = a.configuration_state().await.unwrap();
    let second = b.configuration_state().await.unwrap();
    let events_a = a.events().await.unwrap();
    let events_b = b.events().await.unwrap();
    // Edits visit boards in id order: fail on the last, after the first board's event.
    sqlx::raw_sql("CREATE FUNCTION devagotchi.refuse_workflow_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.board_id=(SELECT max(id) FROM devagotchi.board_projects) THEN RAISE EXCEPTION 'test storage refusal'; END IF; RETURN NEW; END $$; CREATE TRIGGER refuse_workflow_write BEFORE UPDATE ON devagotchi.board_settings FOR EACH ROW EXECUTE FUNCTION devagotchi.refuse_workflow_write();")
        .execute(test.store.admin()).await.unwrap();
    let mut changed = published.workflow;
    changed.name = "Must roll back".into();
    assert!(
        a.edit_shared_workflow(&human, "delivery", 1, &changed)
            .await
            .is_err()
    );
    let failing = if a.board_id() > b.board_id() { &a } else { &b };
    assert!(failing.detach_workflow(&human, 2).await.is_err());
    assert_eq!(a.configuration_state().await.unwrap(), first);
    assert_eq!(b.configuration_state().await.unwrap(), second);
    assert_eq!(a.shared_workflows().await.unwrap()[0].version, 1);
    assert_eq!(
        b.workflow_configuration()
            .await
            .unwrap()
            .shared_workflow
            .unwrap()
            .id,
        "delivery"
    );
    assert_eq!(a.events().await.unwrap(), events_a);
    assert_eq!(b.events().await.unwrap(), events_b);
    sqlx::query("DROP TRIGGER refuse_workflow_write ON devagotchi.board_settings")
        .execute(test.store.admin())
        .await
        .unwrap();
    a.edit_shared_workflow(&human, "delivery", 1, &changed)
        .await
        .unwrap();
    a.verify_chain().await.unwrap();
    b.verify_chain().await.unwrap();
}
