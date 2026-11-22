//! October store/edit.go: version checks do not replace lease fencing on metadata edits.

use super::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn stale_note_fences_are_refused_even_after_the_writer_loses_its_lease() {
    let f = or_skip!(fixture());
    let worker = Actor::agent("worker");
    let next = Actor::agent("next");
    let id = f.new_ticket(&worker, "notes follow the fence");
    let old = f.board.claim(&worker, &id, None).unwrap();
    f.advance(86_400_000);
    let current = f.board.claim(&next, &id, None).unwrap();
    let events = f.board.events(0, 100).unwrap();
    let before = f.board.get(&id).unwrap();
    let error = f
        .board
        .add_note(&worker, &id, "note", "stale result", None, Some(old.fence))
        .unwrap_err();
    assert_eq!(error.code, "stale_fence");
    assert_eq!(f.board.get(&id).unwrap().notes, before.notes);
    assert_eq!(f.board.events(0, 100).unwrap(), events);
    // A separate unfenced observation remains allowed; it claims no current lease.
    f.board
        .add_note(&worker, &id, "note", "outside observation", None, None)
        .unwrap();
    f.board
        .add_note(
            &next,
            &id,
            "note",
            "current worker",
            None,
            Some(current.fence),
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn held_update_requires_current_fence_and_preserves_lease() {
    let f = or_skip!(fixture());
    let owner = Actor::human("owner");
    let worker = Actor::agent("worker");
    let other = Actor::agent("other");
    let id = f.new_ticket(&owner, "claimed work");
    let lease = f.board.claim(&worker, &id, None).unwrap();
    let events = f.board.events(0, 100).unwrap();
    for (actor, fence, expected) in [
        (&worker, None, "stale_fence"),
        (&worker, Some(lease.fence - 1), "stale_fence"),
        (&other, Some(lease.fence), "not_holder"),
        (&other, None, "not_holder"),
    ] {
        let error = f
            .board
            .update(
                actor,
                &id,
                lease.ticket.version,
                Patch {
                    fence,
                    title: Some("must not land".into()),
                    progress: Some(json!({"step": "must not land"})),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(error.code, expected);
        assert_eq!(f.board.get(&id).unwrap().ticket, lease.ticket);
        assert_eq!(f.board.events(0, 100).unwrap(), events);
    }
    let changed = f
        .board
        .update(
            &worker,
            &id,
            lease.ticket.version,
            Patch {
                fence: Some(lease.fence),
                title: Some("actual progress".into()),
                progress: Some(json!({"step": 2})),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(changed.title, "actual progress");
    assert_eq!(changed.progress, Some(json!({"step": 2})));
    assert_eq!(changed.version, lease.ticket.version + 1);
    assert_eq!(changed.lease_expires_at, lease.lease_expires_at.into());
    assert_eq!(changed.heartbeat_at, lease.ticket.heartbeat_at);
    assert_eq!(changed.fence, lease.fence);
    assert_eq!(changed.holder.as_deref(), Some("worker"));
    let events = f.board.events(0, 100).unwrap();
    let event = events.last().unwrap();
    assert_eq!(event.action, crate::actions::TICKET_UPDATED);
    assert_eq!(event.fence, Some(lease.fence));
    assert_eq!(event.data["progress"], true);
    f.board.verify_chain().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn reclaimed_ticket_refuses_previous_worker_even_with_fresh_version() {
    let f = or_skip!(fixture());
    let owner = Actor::human("owner");
    let old = Actor::agent("old");
    let next = Actor::agent("next");
    let id = f.new_ticket(&owner, "reclaimed work");
    let first = f.board.claim(&old, &id, None).unwrap();
    f.advance(86_400_000);
    let current = f.board.claim(&next, &id, None).unwrap();
    assert!(current.fence > first.fence);
    let events = f.board.events(0, 100).unwrap();
    let error = f
        .board
        .update(
            &old,
            &id,
            current.ticket.version,
            Patch {
                fence: Some(first.fence),
                description: Some("late worker overwrite".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code, "stale_fence");
    assert_eq!(f.board.get(&id).unwrap().ticket, current.ticket);
    assert_eq!(f.board.events(0, 100).unwrap(), events);

    // Existing human metadata administration stays available, without moving the lease.
    let edited = f
        .board
        .update(
            &owner,
            &id,
            current.ticket.version,
            Patch {
                description: Some("owner correction".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(edited.holder, current.ticket.holder);
    assert_eq!(edited.lease_expires_at, current.ticket.lease_expires_at);
    assert_eq!(edited.progress, current.ticket.progress);
    let error = f
        .board
        .update(
            &next,
            &id,
            current.ticket.version,
            Patch {
                fence: Some(current.fence),
                title: Some("stale version".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code, "version_conflict");
}

#[tokio::test(flavor = "multi_thread")]
async fn unheld_update_accepts_progress_but_not_a_supplied_stale_fence() {
    let f = or_skip!(fixture());
    let actor = Actor::agent("worker");
    let id = f.new_ticket(&actor, "queued work");
    let original = f.board.get(&id).unwrap().ticket;
    let error = f
        .board
        .update(
            &actor,
            &id,
            original.version,
            Patch {
                fence: Some(original.fence + 1),
                title: Some("wrong fence".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code, "stale_fence");
    let changed = f
        .board
        .update(
            &actor,
            &id,
            original.version,
            Patch {
                progress: Some(json!({"planning": true})),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(changed.progress, Some(json!({"planning": true})));
    assert!(changed.holder.is_none());
    let cleared = f
        .board
        .update(
            &actor,
            &id,
            changed.version,
            Patch {
                progress: Some(serde_json::Value::Null),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(cleared.progress.is_none() || cleared.progress == Some(serde_json::Value::Null));
}
