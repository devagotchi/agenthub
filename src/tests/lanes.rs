//! Opening a board, ids, and the lanes a ticket moves through: claims, leases, expiry, readiness, assignment, WIP, hand-off, force-release.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn open_seeds_metadata_settings_and_workflow_once() {
    let test = or_skip!(super::postgres());
    let board = crate::open_board(&test.url, "devagotchi", true)
        .await
        .expect("open");
    assert_eq!(board.prefix(), "deva");
    assert_eq!(board.settings().expect("settings"), Settings::default());
    assert_eq!(
        board.workflow().expect("workflow"),
        Workflow::default_workflow()
    );
    let id = board
        .create(
            &Actor::human("jesse"),
            NewTicket {
                title: "kept".into(),
                ..NewTicket::default()
            },
        )
        .expect("create")
        .id;
    let events = board.events(0, 100).expect("events");
    drop(board);
    // Opened again (and made again by a second `board init`): the same board, as it was.
    let again = crate::open_board(&test.url, "devagotchi", false)
        .await
        .expect("reopen");
    assert_eq!(again.prefix(), "deva");
    assert_eq!(again.get(&id).expect("kept").ticket.title, "kept");
    drop(again);
    let again = crate::open_board(&test.url, "devagotchi", true)
        .await
        .expect("init again");
    assert_eq!(again.events(0, 100).expect("events"), events);
    // Another project is another board: never this one under a new name, and never made by
    // opening it.
    let missing = match crate::open_board(&test.url, "other", false).await {
        Ok(_) => panic!("a board nobody made must not open"),
        Err(error) => error,
    };
    assert_eq!(missing.code, "no_board");
    let other = crate::open_board(&test.url, "other", true)
        .await
        .expect("made");
    assert_eq!(other.prefix(), "othe");
    assert_eq!(other.get(&id).expect_err("not here").code, "not_found");
}

#[tokio::test(flavor = "multi_thread")]
async fn ids_are_prefixed_hashes_and_children_are_hierarchical() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let epic = f
        .board
        .create(
            &jesse,
            NewTicket {
                title: "Board in the Rust hub".into(),
                kind: Some("epic".into()),
                ..NewTicket::default()
            },
        )
        .expect("epic");
    assert!(epic.id.starts_with("deva-"), "{}", epic.id);
    assert_eq!(epic.id.len(), "deva-".len() + 4);
    let child_a = f
        .board
        .create(
            &jesse,
            NewTicket {
                title: "store".into(),
                parent: Some(epic.id.clone()),
                ..NewTicket::default()
            },
        )
        .expect("child a");
    let child_b = f
        .board
        .create(
            &jesse,
            NewTicket {
                title: "mcp tools".into(),
                parent: Some(epic.id.clone()),
                ..NewTicket::default()
            },
        )
        .expect("child b");
    assert_eq!(child_a.id, format!("{}.1", epic.id));
    assert_eq!(child_b.id, format!("{}.2", epic.id));
    let grandchild = f
        .board
        .create(
            &jesse,
            NewTicket {
                title: "schema".into(),
                parent: Some(child_a.id.clone()),
                ..NewTicket::default()
            },
        )
        .expect("grandchild");
    assert_eq!(grandchild.id, format!("{}.1.1", epic.id));
    let view = f.board.get(&epic.id).expect("view");
    assert_eq!(view.children, vec![child_a.id.clone(), child_b.id.clone()]);
    assert_eq!(view.dependents.len(), 2);
    assert!(view.dependents.iter().all(|d| d.kind == DepKind::Parent));
    // Hash ids from the same actor and title in the same millisecond still differ.
    let one = f.new_ticket(&jesse, "same title");
    let two = f.new_ticket(&jesse, "same title");
    assert_ne!(one, two);
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_heartbeat_complete_moves_through_the_lanes() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let claude = Actor::agent("claude");
    let codex = Actor::agent("codex");
    let id = f.new_ticket(&jesse, "port the claim protocol");

    let lease = f.board.claim(&claude, &id, None).expect("claim");
    assert_eq!(lease.ticket.state, "in_progress");
    assert_eq!(lease.ticket.holder.as_deref(), Some("claude"));
    assert_eq!(lease.fence, 1);
    assert_eq!(
        lease.heartbeat_every_ms,
        Settings::default().lease_ttl_ms / 3
    );
    assert!(!lease.already_held);

    let again = f
        .board
        .claim(&claude, &id, None)
        .expect("re-claim is idempotent");
    assert!(again.already_held);
    assert_eq!(again.fence, 1);

    let other = f.board.claim(&codex, &id, None).expect_err("held by other");
    assert_eq!(other.code, "held_by_other");

    f.advance(60_000);
    let beat = f
        .board
        .heartbeat(
            &claude,
            &id,
            lease.fence,
            Some(serde_json::json!({ "step": "tests" })),
        )
        .expect("heartbeat");
    assert_eq!(
        beat.lease_expires_at,
        beat.ticket.heartbeat_at.expect("beaten") + Settings::default().lease_ttl_ms,
        "a beat extends the lease by its ttl from now"
    );
    assert!(beat.lease_expires_at >= lease.lease_expires_at);
    assert_eq!(
        beat.ticket.progress,
        Some(serde_json::json!({ "step": "tests" }))
    );

    let stale = f
        .board
        .heartbeat(&claude, &id, 99, None)
        .expect_err("wrong fence");
    assert_eq!(stale.code, "stale_fence");
    let not_holder = f
        .board
        .complete(&codex, &id, lease.fence, "nope")
        .expect_err("codex does not hold it");
    assert_eq!(not_holder.code, "not_holder");

    let reviewed = f
        .board
        .complete(&claude, &id, lease.fence, "store and tests landed")
        .expect("complete");
    assert_eq!(reviewed.state, "review");
    assert_eq!(reviewed.holder, None);
    assert_eq!(reviewed.last_worker.as_deref(), Some("claude"));
    assert_eq!(reviewed.closed_at, None);

    // The author may not review their own work; the lane says so as data.
    let same = f
        .board
        .claim(&claude, &id, None)
        .expect_err("distinct reviewer");
    assert_eq!(same.code, "distinct_required");
    let review = f.board.claim(&codex, &id, None).expect("codex reviews");
    assert_eq!(review.ticket.state, "reviewing");
    assert_eq!(review.fence, 2, "fences never repeat");
    let done = f
        .board
        .complete(&codex, &id, review.fence, "looks right")
        .expect("review complete");
    assert_eq!(done.state, "done");
    assert!(done.closed_at.is_some());

    let view = f.board.get(&id).expect("view");
    let kinds: Vec<&str> = view.notes.iter().map(|n| n.kind.as_str()).collect();
    assert_eq!(kinds, vec!["summary", "summary"]);
    let chain = f.board.verify_chain().expect("verify");
    assert!(chain.ok);
    assert_eq!(
        chain.entries, 6,
        "created, claimed, heartbeat, completed, claimed, completed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_lease_goes_back_to_ready_and_the_old_fence_is_rejected() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let claude = Actor::agent("claude");
    let codex = Actor::agent("codex");
    let id = f.new_ticket(&jesse, "something slow");
    let lease = f.board.claim(&claude, &id, Some(60_000)).expect("claim");
    assert_eq!(lease.ticket.lease_ttl_ms, Some(60_000));

    f.advance(60_001);
    let lapsed = f.board.get(&id).expect("get").ticket;
    assert!(lapsed.lease_expired, "lapsed but not yet reaped");

    // Codex claims it; the expiry happens lazily inside the claim and the fence moves on.
    let taken = f
        .board
        .claim(&codex, &id, None)
        .expect("claim after expiry");
    assert_eq!(taken.fence, 2);
    assert_eq!(taken.ticket.expiry_count, 1);
    let late = f
        .board
        .heartbeat(&claude, &id, lease.fence, None)
        .expect_err("the woken-up old holder is rejected");
    assert_eq!(late.code, "stale_fence");
    let late_complete = f
        .board
        .complete(&claude, &id, lease.fence, "done?")
        .expect_err("same for complete");
    assert_eq!(late_complete.code, "stale_fence");
}

#[tokio::test(flavor = "multi_thread")]
async fn reaper_sends_a_ticket_to_attention_after_too_many_expiries() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let agent = Actor::agent("flaky");
    let id = f.new_ticket(&jesse, "keeps dying");
    for expiry in 1..=3 {
        f.board.claim(&agent, &id, Some(30_000)).expect("claim");
        f.advance(30_001);
        assert_eq!(f.board.reap().expect("reap"), 1);
        let ticket = f.board.get(&id).expect("get").ticket;
        assert_eq!(ticket.expiry_count, expiry);
        if expiry < 3 {
            assert_eq!(ticket.state, "ready");
        } else {
            assert_eq!(ticket.state, "needs_attention");
        }
    }
    assert_eq!(f.board.reap().expect("nothing left"), 0);
    let not_claimable = f
        .board
        .claim(&agent, &id, None)
        .expect_err("attention is not a queue");
    assert_eq!(not_claimable.code, "not_claimable");
    // A person puts it back.
    let ticket = f.board.get(&id).expect("get").ticket;
    let back = f
        .board
        .update(
            &jesse,
            &id,
            ticket.version,
            Patch {
                state: Some("ready".into()),
                ..Patch::default()
            },
        )
        .expect("back to ready");
    assert_eq!(back.state, "ready");
}

#[tokio::test(flavor = "multi_thread")]
async fn ready_excludes_blocked_tickets_until_the_blocker_is_done() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let claude = Actor::agent("claude");
    let schema = f.new_ticket(&jesse, "schema");
    let tools = f.new_ticket(&jesse, "mcp tools");
    f.board
        .add_dep(&jesse, &tools, &schema, DepKind::Blocks)
        .expect("dep");
    let cycle = f
        .board
        .add_dep(&jesse, &schema, &tools, DepKind::Blocks)
        .expect_err("cycle refused");
    assert_eq!(cycle.code, "dependency_cycle");

    let ready: Vec<String> = f
        .board
        .ready(&claude, 10)
        .expect("ready")
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ready, vec![schema.clone()]);
    let blocked = f.board.claim(&claude, &tools, None).expect_err("blocked");
    assert_eq!(blocked.code, "blocked");

    let next = f
        .board
        .claim_next(&claude, &NextFilter::default())
        .expect("claim_next")
        .expect("something ready");
    assert_eq!(next.ticket.id, schema);
    f.board
        .complete(&claude, &schema, next.fence, "schema landed")
        .expect("complete");
    // Review is not done: still blocking.
    assert!(
        f.board
            .get(&tools)
            .expect("get")
            .ticket
            .blocked_by
            .contains(&schema)
    );
    let review = f
        .board
        .claim(&Actor::agent("codex"), &schema, None)
        .expect("review");
    f.board
        .complete(&Actor::agent("codex"), &schema, review.fence, "ok")
        .expect("done");
    assert!(
        f.board
            .get(&tools)
            .expect("get")
            .ticket
            .blocked_by
            .is_empty()
    );
    let next = f
        .board
        .claim_next(&claude, &NextFilter::default())
        .expect("claim_next")
        .expect("tools now ready");
    assert_eq!(next.ticket.id, tools);
}

#[tokio::test(flavor = "multi_thread")]
async fn assigned_tickets_go_to_their_assignee_first_and_only() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let claude = Actor::agent("claude");
    let codex = Actor::agent("codex");
    let anyone = f.new_ticket(&jesse, "anyone");
    let mine = f
        .board
        .create(
            &jesse,
            NewTicket {
                title: "for codex".into(),
                assignee: Some("codex".into()),
                priority: Some(3),
                ..NewTicket::default()
            },
        )
        .expect("create")
        .id;
    let claude_sees: Vec<String> = f
        .board
        .ready(&claude, 10)
        .expect("ready")
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(claude_sees, vec![anyone.clone()]);
    let codex_next = f
        .board
        .claim_next(&codex, &NextFilter::default())
        .expect("claim_next")
        .expect("ready");
    assert_eq!(
        codex_next.ticket.id, mine,
        "assigned work outranks a better priority"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn wip_limit_stops_a_second_claim() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let one = f.new_ticket(&jesse, "one");
    let two = f.new_ticket(&jesse, "two");
    let three = f.new_ticket(&jesse, "three");
    let agent = Actor::agent("claude");
    f.board.claim(&agent, &one, None).expect("first");
    f.board
        .claim(&agent, &two, None)
        .expect("second (default limit is 2)");
    let third = f
        .board
        .claim(&agent, &three, None)
        .expect_err("over the limit");
    assert_eq!(third.code, "wip_limit");
    let none = f
        .board
        .claim_next(&agent, &NextFilter::default())
        .expect_err("claim_next also checks WIP");
    assert_eq!(none.code, "wip_limit");
    let generous = Actor {
        id: "claude",
        human: false,
        policy: None,
        wip_limit: Some(0),
    };
    f.board
        .claim(&generous, &three, None)
        .expect("0 = unlimited");
}

#[tokio::test(flavor = "multi_thread")]
async fn handoff_notes_releases_and_assigns_in_one_step() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let claude = Actor::agent("claude");
    let id = f.new_ticket(&jesse, "cross-vendor");
    let lease = f.board.claim(&claude, &id, None).expect("claim");
    let empty = f
        .board
        .handoff(&claude, &id, lease.fence, "   ", Some("codex"))
        .expect_err("a handoff needs a note");
    assert_eq!(empty.code, "invalid");
    let handed = f
        .board
        .handoff(
            &claude,
            &id,
            lease.fence,
            "tests are red on Windows; see note 1",
            Some("codex"),
        )
        .expect("handoff");
    assert_eq!(handed.state, "ready");
    assert_eq!(handed.holder, None);
    assert_eq!(handed.assignee.as_deref(), Some("codex"));
    let view = f.board.get(&id).expect("view");
    assert_eq!(view.notes.len(), 1);
    assert_eq!(view.notes[0].kind, "handoff");
    assert_eq!(view.notes[0].to_principal.as_deref(), Some("codex"));
    assert_eq!(view.notes[0].fence, Some(lease.fence));
}

#[tokio::test(flavor = "multi_thread")]
async fn force_release_is_for_humans_with_a_reason() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let claude = Actor::agent("claude");
    let id = f.new_ticket(&jesse, "stuck");
    f.board.claim(&claude, &id, None).expect("claim");
    let agent = f
        .board
        .force_release(&Actor::agent("codex"), &id, "mine now")
        .expect_err("agents may not");
    assert_eq!(agent.code, "humans_only");
    let short = f
        .board
        .force_release(&jesse, &id, "x")
        .expect_err("reason too short");
    assert_eq!(short.code, "invalid");
    let freed = f
        .board
        .force_release(&jesse, &id, "claude's session died")
        .expect("forced");
    assert_eq!(freed.holder, None);
    assert_eq!(freed.state, "ready");
    let events = f.board.events(0, 100).expect("events");
    let forced = events
        .iter()
        .find(|e| e.action == "ticket.force_released")
        .expect("event");
    assert_eq!(forced.data["previous_holder"], "claude");
    assert_eq!(forced.data["previous_fence"], 1);
}
