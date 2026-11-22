//! October ops.go lifecycle behavior missing from the first Rust port.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn handoff_records_last_worker_and_requires_a_distinct_next_reviewer() {
    let f = or_skip!(fixture());
    let author = Actor::agent("author");
    let reviewer = Actor::agent("reviewer");
    let next = Actor::agent("next");
    let id = f.new_ticket(&author, "review handoff");
    let work = f.board.claim(&author, &id, None).unwrap();
    f.board
        .complete(&author, &id, work.fence, "implemented")
        .unwrap();
    let review = f.board.claim(&reviewer, &id, None).unwrap();
    let handed = f
        .board
        .handoff(
            &reviewer,
            &id,
            review.fence,
            "review in progress",
            Some("next"),
        )
        .unwrap();
    assert_eq!(handed.last_worker.as_deref(), Some("reviewer"));
    assert_eq!(handed.assignee.as_deref(), Some("next"));
    assert_eq!(handed.state, "review");
    assert!(handed.holder.is_none());
    assert_eq!(
        f.board.claim(&reviewer, &id, None).unwrap_err().code,
        "distinct_required"
    );
    f.board.claim(&next, &id, None).unwrap();
    assert!(f.board.verify_chain().unwrap().ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn completing_without_an_active_lane_refuses_without_fabricating_completion() {
    let f = or_skip!(fixture());
    let actor = Actor::agent("worker");
    let id = f.new_ticket(&actor, "damaged workflow");
    let lease = f.board.claim(&actor, &id, None).unwrap();
    let mut workflow = f.board.workflow().unwrap();
    workflow.lanes.clear();
    assert!(
        f.board
            .put_workflow(&Actor::human("owner"), &workflow)
            .is_err()
    );
    // Inject a damaged stored definition; public configuration already rejects this.
    sqlx::query("UPDATE devagotchi.board_settings SET value=$3 WHERE tenant_id=$1 AND board_id=$2 AND key='workflow'")
        .bind(LOCAL_TENANT).bind(PROJECT).bind(serde_json::to_string(&workflow).unwrap())
        .execute(f.test.store.admin()).await.unwrap();
    let events = f.board.events(0, 100).unwrap();
    let before = f.board.get(&id).unwrap();
    let error = f
        .board
        .complete(&actor, &id, lease.fence, "must not be recorded")
        .unwrap_err();
    assert_eq!(error.code, "not_completable");
    let after = f.board.get(&id).unwrap();
    assert_eq!(after.ticket, before.ticket);
    assert_eq!(after.notes, before.notes);
    assert_eq!(f.board.events(0, 100).unwrap(), events);
}

#[tokio::test(flavor = "multi_thread")]
async fn force_release_keeps_a_human_note_and_agent_delegation_cannot_bypass_the_fence() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let worker = Actor::agent("worker");
    let intruder = Actor::agent("intruder");
    let id = f.new_ticket(&human, "recover work");
    let held = f.board.claim(&worker, &id, None).unwrap();
    let events = f.board.events(0, 100).unwrap();
    assert_eq!(
        f.board
            .delegate(&intruder, &id, held.ticket.version, "intruder", "take over")
            .unwrap_err()
            .code,
        "humans_only"
    );
    assert_eq!(f.board.events(0, 100).unwrap(), events);
    let released = f
        .board
        .force_release(&human, &id, "worker unavailable")
        .unwrap();
    assert!(released.holder.is_none());
    let view = f.board.get(&id).unwrap();
    let note = view.notes.last().unwrap();
    assert_eq!(note.author, "owner");
    assert_eq!(note.body, "Force-released from worker: worker unavailable");
    assert!(f.board.verify_chain().unwrap().ok);
}
