//! Versioned configuration uses the same board transaction as tickets and approvals.
use super::{fixture, or_skip};
use crate::hosted::HostedBoard;
use crate::{Actor, ProjectSettings, SettingsEdit, StateKind, Workflow};

#[tokio::test(flavor = "multi_thread")]
async fn settings_versions_preserve_approvals_and_reject_stale_edits() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let initial = f.board.configuration_state().unwrap();
    assert_eq!((initial.settings_version, initial.workflow_version), (1, 1));
    let edit = SettingsEdit::from(&initial.settings);
    let events = f.board.events(0, 1000).unwrap();
    assert_eq!(f.board.edit_settings(&human, 1, &edit).unwrap(), initial);
    assert_eq!(f.board.events(0, 1000).unwrap(), events);
    assert_eq!(
        f.board
            .edit_settings(&Actor::agent("agent"), 1, &edit)
            .unwrap_err()
            .code,
        "humans_only"
    );
    f.board
        .grant_standing(&human, "restart", Some("demo"), "restart api", 30)
        .unwrap();
    assert_eq!(
        f.board.edit_settings(&human, 1, &edit).unwrap_err().code,
        "version_conflict"
    );
    let granted = f.board.configuration_state().unwrap();
    assert_eq!(granted.settings_version, 2);
    let mut changed = SettingsEdit::from(&granted.settings);
    changed.default_priority = 7;
    let saved = f.board.edit_settings(&human, 2, &changed).unwrap();
    assert_eq!(saved.settings_version, 3);
    assert_eq!(saved.settings.standing, granted.settings.standing);
    assert_eq!(saved.workflow_version, 1);
    f.board
        .set_project(
            &human,
            &ProjectSettings {
                repo: Some("https://github.com/acme/demo".into()),
                environments: None,
            },
        )
        .unwrap();
    assert_eq!(
        f.board.edit_settings(&human, 3, &changed).unwrap_err().code,
        "version_conflict"
    );
    let current = f.board.configuration_state().unwrap();
    assert_eq!(current.settings_version, 4);
    f.board.put_settings(&human, &current.settings).unwrap();
    assert_eq!(f.board.configuration_state().unwrap(), current);
    let mut replacement = current.settings.clone();
    replacement.default_priority = 8;
    f.board.put_settings(&human, &replacement).unwrap();
    assert_eq!(f.board.configuration_state().unwrap().settings_version, 5);
    f.board.verify_chain().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_settings_roll_back_and_valid_settings_control_real_claims() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let initial = f.board.configuration_state().unwrap();
    let events = f.board.events(0, 1000).unwrap();
    let base = SettingsEdit::from(&initial.settings);
    let mut invalid = Vec::new();
    let mut edit = base.clone();
    edit.lease_min_ttl_ms = 0;
    invalid.push(edit);
    let mut edit = base.clone();
    edit.lease_ttl_ms = 1;
    invalid.push(edit);
    let mut edit = base.clone();
    edit.lease_max_ttl_ms = i64::MAX;
    invalid.push(edit);
    let mut edit = base.clone();
    edit.wip_limit_default = -1;
    invalid.push(edit);
    let mut edit = base.clone();
    edit.max_lease_expiries = -1;
    invalid.push(edit);
    let mut edit = base.clone();
    edit.repo = Some("file:///private".into());
    invalid.push(edit);
    let mut edit = base.clone();
    edit.environments = vec!["dev".into(), "dev".into()];
    invalid.push(edit);
    for edit in invalid {
        assert!(f.board.edit_settings(&human, 1, &edit).is_err());
        assert_eq!(f.board.configuration_state().unwrap(), initial);
        assert_eq!(f.board.events(0, 1000).unwrap(), events);
    }
    let mut edit = base;
    edit.wip_limit_default = 1;
    edit.default_priority = 6;
    edit.lease_ttl_ms = 60_000;
    f.board.edit_settings(&human, 1, &edit).unwrap();
    let first = f.new_ticket(&human, "first");
    let second = f.new_ticket(&human, "second");
    assert_eq!(f.board.get(&first).unwrap().ticket.priority, 6);
    let agent = Actor::agent("worker");
    let lease = f.board.claim(&agent, &first, None).unwrap();
    assert_eq!(lease.heartbeat_every_ms, 20_000);
    assert_eq!(
        f.board.claim(&agent, &second, None).unwrap_err().code,
        "wip_limit"
    );
    f.board
        .release(&agent, &first, lease.fence, "next task")
        .unwrap();
    f.board.claim(&agent, &second, None).unwrap();
    f.board.verify_chain().unwrap();
}

#[tokio::test]
async fn concurrent_configuration_edits_have_one_winner_and_do_not_cross_boards() {
    let test = or_skip!(super::postgres());
    let tenant = "t-00000000-0000-4000-8000-000000000001";
    let board = HostedBoard::new(test.store.clone(), tenant, "demo").unwrap();
    board.initialize().await.unwrap();
    let another = HostedBoard::new(test.store.clone(), tenant, "demo").unwrap();
    let other_board = HostedBoard::new(test.store.clone(), tenant, "other").unwrap();
    other_board.initialize().await.unwrap();
    let other_tenant = HostedBoard::new(
        test.store.clone(),
        "t-00000000-0000-4000-8000-000000000002",
        "demo",
    )
    .unwrap();
    other_tenant.initialize().await.unwrap();
    let initial = board.configuration_state().await.unwrap();
    let mut first = SettingsEdit::from(&initial.settings);
    let mut second = first.clone();
    first.default_priority = 5;
    second.default_priority = 9;
    let human = Actor::human("owner");
    let (a, b) = tokio::join!(
        board.edit_settings(&human, 1, &first),
        another.edit_settings(&human, 1, &second)
    );
    let (winner, loser) = if a.is_ok() {
        (a.unwrap(), b.unwrap_err())
    } else {
        (b.unwrap(), a.unwrap_err())
    };
    assert_eq!(loser.code, "version_conflict");
    assert_eq!(winner.settings_version, 2);
    assert_eq!(board.configuration_state().await.unwrap(), winner);
    assert_eq!(other_board.configuration_state().await.unwrap(), initial);
    assert_eq!(other_tenant.configuration_state().await.unwrap(), initial);
    board.verify_chain().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn workflow_edits_keep_live_tasks_meaningful_and_leases_usable() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let agent = Actor::agent("worker");
    let id = f.new_ticket(&human, "held during edit");
    let lease = f.board.claim(&agent, &id, None).unwrap();
    let initial = f.board.configuration_state().unwrap();
    assert_eq!(
        f.board
            .edit_workflow(&agent, 1, &initial.workflow)
            .unwrap_err()
            .code,
        "humans_only"
    );
    let events = f.board.events(0, 1000).unwrap();
    assert_eq!(
        f.board.edit_workflow(&human, 1, &initial.workflow).unwrap(),
        initial
    );
    assert_eq!(f.board.events(0, 1000).unwrap(), events);
    // A structurally valid workflow still cannot reinterpret a held state.
    let mut invalid = initial.workflow.clone();
    invalid
        .states
        .iter_mut()
        .find(|s| s.key == "in_progress")
        .unwrap()
        .kind = StateKind::Queue;
    invalid.lanes.remove(0);
    assert!(invalid.validate().is_ok());
    assert_eq!(
        f.board.edit_workflow(&human, 1, &invalid).unwrap_err().code,
        "invalid"
    );
    invalid.states.retain(|s| s.key != "in_progress");
    assert!(invalid.validate().is_ok());
    assert_eq!(
        f.board.edit_workflow(&human, 1, &invalid).unwrap_err().code,
        "invalid"
    );
    assert_eq!(f.board.configuration_state().unwrap(), initial);
    assert_eq!(f.board.events(0, 1000).unwrap(), events);
    let mut changed = initial.workflow.clone();
    changed
        .states
        .iter_mut()
        .find(|s| s.key == "in_progress")
        .unwrap()
        .label = "Building".into();
    let saved = f.board.edit_workflow(&human, 1, &changed).unwrap();
    assert_eq!(saved.workflow_version, 2);
    assert_eq!(saved.settings_version, 1);
    assert_eq!(
        f.board
            .edit_workflow(&human, 1, &initial.workflow)
            .unwrap_err()
            .code,
        "version_conflict"
    );
    let done = f
        .board
        .complete(&agent, &id, lease.fence, "implemented")
        .unwrap();
    assert_eq!(done.state, "review");
    changed.name = "Team workflow".into();
    f.board.put_workflow(&human, &changed).unwrap();
    assert_eq!(f.board.configuration_state().unwrap().workflow_version, 3);
    f.board.put_workflow(&human, &changed).unwrap();
    assert_eq!(f.board.configuration_state().unwrap().workflow_version, 3);
    f.board.verify_chain().unwrap();
}

#[test]
fn workflow_configuration_rejects_unrunnable_shapes() {
    let base = Workflow::default_workflow();
    assert!(base.validate().is_ok());
    let mut invalid = Vec::new();
    let mut w = base.clone();
    w.states.push(w.states[0].clone());
    invalid.push(w);
    let mut w = base.clone();
    w.lanes.push(w.lanes[0].clone());
    invalid.push(w);
    let mut w = base.clone();
    w.transitions.push(w.transitions[0].clone());
    invalid.push(w);
    let mut w = base.clone();
    w.initial_state = "in_progress".into();
    invalid.push(w);
    let mut w = base.clone();
    w.initial_state = "done".into();
    invalid.push(w);
    let mut w = base.clone();
    w.attention_state = "ready".into();
    invalid.push(w);
    let mut w = base.clone();
    w.lanes[0].complete_to = "reviewing".into();
    invalid.push(w);
    let mut w = base.clone();
    w.lanes[0].release_to = "done".into();
    invalid.push(w);
    let mut w = base.clone();
    w.lanes.remove(0);
    invalid.push(w);
    let mut w = base.clone();
    w.transitions[0].to = "in_progress".into();
    invalid.push(w);
    let mut w = base.clone();
    w.transitions[0].from = "in_progress".into();
    invalid.push(w);
    let mut w = base.clone();
    w.states[0].label = "\n".into();
    invalid.push(w);
    for workflow in invalid {
        assert!(workflow.validate().is_err(), "{workflow:?}");
    }
}
