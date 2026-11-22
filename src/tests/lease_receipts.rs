use super::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lease_replays_preserve_original_fences_expiry_and_single_events() {
    let f = or_skip!(fixture());
    let actor = Actor::human("owner");
    let id = f.new_ticket(&actor, "Work");
    let requested = |key: &str, operation: &str, input: serde_json::Value| {
        f.board.requested("owner", key, operation, &input).unwrap()
    };
    let claim = requested("claim-1", "claim", json!({"id":id}));
    let first = claim.claim(&actor, &id, None).unwrap();
    let heartbeat = requested("beat-1", "heartbeat", json!({"id":id,"fence":first.fence}));
    let beat = heartbeat.heartbeat(&actor, &id, first.fence, None).unwrap();
    let release = requested(
        "release-1",
        "release",
        json!({"id":id,"fence":first.fence,"reason":"Pause"}),
    );
    let released = release.release(&actor, &id, first.fence, "Pause").unwrap();
    let second = f.board.claim(&actor, &id, None).unwrap();
    assert!(second.fence > first.fence);
    let count = f.board.events(0, 100).unwrap().len();
    assert_eq!(claim.claim(&actor, &id, None).unwrap(), first);
    assert_eq!(
        heartbeat.heartbeat(&actor, &id, first.fence, None).unwrap(),
        beat
    );
    assert_eq!(
        release.release(&actor, &id, first.fence, "Pause").unwrap(),
        released
    );
    assert_eq!(
        f.board.get(&id).unwrap().ticket,
        second.ticket,
        "old receipt cannot release or renew a newer lease"
    );
    assert_eq!(f.board.events(0, 100).unwrap().len(), count);
    let handoff = requested(
        "handoff-1",
        "handoff",
        json!({"id":id,"fence":second.fence,"note":"Continue"}),
    );
    let handed = handoff
        .handoff(&actor, &id, second.fence, "Continue", None)
        .unwrap();
    assert_eq!(
        handoff
            .handoff(&actor, &id, second.fence, "Continue", None)
            .unwrap(),
        handed
    );
    let third = f.board.claim(&actor, &id, None).unwrap();
    let complete = requested(
        "complete-1",
        "complete",
        json!({"id":id,"fence":third.fence,"summary":"Ready"}),
    );
    let done = complete
        .complete(&actor, &id, third.fence, "Ready")
        .unwrap();
    assert_eq!(
        complete
            .complete(&actor, &id, third.fence, "Ready")
            .unwrap(),
        done
    );
    assert_eq!(
        f.board.get(&id).unwrap().notes.len(),
        2,
        "one handoff and one summary"
    );
    let other = f.new_ticket(&actor, "Release forcibly");
    f.board.claim(&actor, &other, None).unwrap();
    let force = requested(
        "force-1",
        "force_release",
        json!({"id":other,"reason":"Operator"}),
    );
    let forced = force.force_release(&actor, &other, "Operator").unwrap();
    assert_eq!(
        force.force_release(&actor, &other, "Operator").unwrap(),
        forced
    );
    assert_eq!(f.board.get(&other).unwrap().notes.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_next_replays_selected_task_or_empty_result_without_claiming_new_work() {
    let f = or_skip!(fixture());
    let actor = Actor::human("owner");
    let empty = f
        .board
        .requested("owner", "empty", "claim_next", &json!({}))
        .unwrap();
    assert!(
        empty
            .claim_next(&actor, &NextFilter::default())
            .unwrap()
            .is_none()
    );
    f.new_ticket(&actor, "First");
    f.new_ticket(&actor, "Second");
    assert!(
        empty
            .claim_next(&actor, &NextFilter::default())
            .unwrap()
            .is_none()
    );
    let next = f
        .board
        .requested("owner", "next", "claim_next", &json!({}))
        .unwrap();
    let selected = next
        .claim_next(&actor, &NextFilter::default())
        .unwrap()
        .unwrap();
    let count = f.board.events(0, 100).unwrap().len();
    assert_eq!(
        next.claim_next(&actor, &NextFilter::default()).unwrap(),
        Some(selected.clone())
    );
    assert!(next.hosted().replayed());
    assert_eq!(f.board.events(0, 100).unwrap().len(), count);
    f.board
        .release(&actor, &selected.ticket.id, selected.fence, "Pause")
        .unwrap();
    assert_eq!(
        next.claim_next(&actor, &NextFilter::default()).unwrap(),
        Some(selected)
    );
}
