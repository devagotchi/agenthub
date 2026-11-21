use super::*;
use crate::policy::{Effect, Identity, Rule, Selector, Subject, catalog};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_policy_uses_current_labels_and_cannot_bypass_fences_or_reviewers() {
    let f = or_skip!(fixture());
    let store = &f.test.store;
    let human = Actor::human("owner");
    let worker = Actor {
        id: "@agent:account:worker",
        human: false,
        wip_limit: Some(2),
        policy: Some(Identity {
            kind: "codex".into(),
            roles: vec!["worker".into()],
        }),
    };
    let roles = vec!["admin".into()];
    let owner = Subject {
        id: "owner",
        human: true,
        kind: "human",
        roles: &roles,
    };
    transaction!(store, |connection| {
        store.scope(connection, LOCAL_TENANT).await?;
        catalog::initialize_in(connection, LOCAL_TENANT).await?;
        catalog::put_rule_in(
            connection,
            LOCAL_TENANT,
            &owner,
            &Rule {
                id: "protected-edits".into(),
                name: "Protected tasks".into(),
                subject: "agent".into(),
                action: "task.edit".into(),
                selector: Selector {
                    projects: vec![PROJECT.into()],
                    labels: vec!["human-only".into()],
                },
                effect: Effect::Deny,
                priority: 100,
                enabled: true,
                version: 0,
            },
        )
        .await?;
        Ok(())
    })
    .unwrap();
    let protected = f
        .board
        .create(
            &human,
            NewTicket {
                title: "Protected".into(),
                labels: vec!["human-only".into()],
                priority: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    let available = f
        .board
        .create(
            &human,
            NewTicket {
                title: "Available".into(),
                priority: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let before = f.board.events(0, 1000).unwrap().len();
    assert_eq!(
        f.board
            .claim(&worker, &protected.id, None)
            .unwrap_err()
            .code,
        "policy_denied"
    );
    assert_eq!(
        f.board
            .update(
                &worker,
                &protected.id,
                protected.version,
                Patch {
                    labels: Some(Vec::new()),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
        "policy_denied"
    );
    assert_eq!(
        f.board
            .add_dep(&worker, &protected.id, &available.id, DepKind::Blocks)
            .unwrap_err()
            .code,
        "policy_denied"
    );
    assert_eq!(f.board.events(0, 1000).unwrap().len(), before);
    let held = f
        .board
        .claim_next(&worker, &NextFilter::default())
        .unwrap()
        .unwrap();
    assert_eq!(held.ticket.id, available.id);
    assert_eq!(
        f.board
            .update(
                &worker,
                &available.id,
                held.ticket.version,
                Patch {
                    title: Some("missing fence".into()),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
        "stale_fence"
    );
    assert_eq!(
        f.board
            .update(
                &worker,
                &available.id,
                held.ticket.version,
                Patch {
                    state: Some("cancelled".into()),
                    fence: Some(held.fence),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .code,
        "policy_denied"
    );
    f.board
        .release(&worker, &available.id, held.fence, "next")
        .unwrap();
    let cleared = f
        .board
        .update(
            &human,
            &protected.id,
            protected.version,
            Patch {
                labels: Some(Vec::new()),
                ..Default::default()
            },
        )
        .unwrap();
    let held = f.board.claim(&worker, &cleared.id, None).unwrap();
    let reviewed = f
        .board
        .complete(&worker, &cleared.id, held.fence, "Ready for review")
        .unwrap();
    assert_eq!(reviewed.state, "review");
    let mut reviewer = worker.clone();
    reviewer.id = "@agent:account:reviewer";
    assert_eq!(
        f.board
            .claim(&reviewer, &cleared.id, None)
            .unwrap_err()
            .code,
        "policy_denied"
    );
    reviewer
        .policy
        .as_mut()
        .unwrap()
        .roles
        .push("reviewer".into());
    assert!(f.board.claim(&reviewer, &cleared.id, None).is_ok());
    assert_eq!(
        f.board
            .force_release(&reviewer, &cleared.id, "Cannot approve")
            .unwrap_err()
            .code,
        "humans_only"
    );
}
