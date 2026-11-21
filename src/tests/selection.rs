//! Source queue filters apply before the candidate cap; list and claim orders differ.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_state_selection_precedes_the_twenty_five_candidate_limit() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    for n in 0..26 {
        f.new_ticket(&human, &format!("Earlier work {n}"));
    }
    let target = f.new_ticket(&human, "Ready for review");
    let worker = Actor::agent("worker");
    let lease = f.board.claim(&worker, &target, None).unwrap();
    f.board
        .complete(&worker, &target, lease.fence, "Review this")
        .unwrap();
    let lease = f
        .board
        .claim_next(
            &Actor::agent("reviewer"),
            &NextFilter {
                states: vec!["review".into()],
                ..Default::default()
            },
        )
        .unwrap()
        .expect("matching work after 26 other ready tasks");
    assert_eq!(lease.ticket.id, target);
    assert_eq!(lease.ticket.state, "reviewing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_requires_every_normalized_label_before_limiting_candidates() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    for n in 0..26 {
        f.board
            .create(
                &human,
                NewTicket {
                    title: format!("Partial match {n}"),
                    labels: vec!["release".into()],
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let target = f
        .board
        .create(
            &human,
            NewTicket {
                title: "All labels".into(),
                labels: vec!["release".into(), "api".into()],
                ..Default::default()
            },
        )
        .unwrap();
    let lease = f
        .board
        .claim_next(
            &Actor::agent("worker"),
            &NextFilter {
                label: Some(" RELEASE ".into()),
                labels: vec!["API".into(), "api".into(), " ".into()],
                ..Default::default()
            },
        )
        .unwrap()
        .expect("all labels match");
    assert_eq!(lease.ticket.id, target.id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_lists_keep_priority_order_while_actor_ready_lists_prefer_assignment() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let worker = Actor::agent("worker");
    let urgent = f
        .board
        .create(
            &human,
            NewTicket {
                title: "Urgent".into(),
                priority: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    let assigned = f
        .board
        .create(
            &human,
            NewTicket {
                title: "Assigned".into(),
                priority: Some(3),
                assignee: Some("worker".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let list = f.board.list(&Filter::default()).unwrap();
    assert_eq!(list[0].id, urgent.id);
    let ready = f.board.ready(&worker, 10).unwrap();
    assert_eq!(ready[0].id, assigned.id);
    let next = f
        .board
        .claim_next(&worker, &NextFilter::default())
        .unwrap()
        .unwrap();
    assert_eq!(next.ticket.id, assigned.id);
}
