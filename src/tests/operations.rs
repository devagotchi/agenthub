//! The operations that used to take a surface two calls, each now one transaction
//! (`.ai/patterns/board-operations.md`, rule 1): asking under a standing grant, approve and
//! allow, delegate, undo-close, a webhook set, a person taking back what they allowed, and
//! create's default repo. Each multi-row one is shown atomic by a refused write.

use super::{Fixture, board_changes, fixture, or_skip, quiet};
use crate::approval::{Ask, MAX_LIVE};
use crate::{Actor, NewTicket, Patch, ProjectSettings, actions};

const WHAT: &str = "systemctl restart web";

fn ask(kind: Option<&'static str>) -> Ask<'static> {
    Ask {
        title: "Restart web",
        what: WHAT,
        why: "it is stuck",
        undo: "nothing to undo",
        kind,
        target: None,
    }
}

/// Refuse every outbox row of `kind` until the guard is dropped: the transaction that
/// appends one fails at its last write.
struct Refuse<'a> {
    fixture: &'a Fixture,
    name: &'static str,
}

impl<'a> Refuse<'a> {
    async fn new(fixture: &'a Fixture, name: &'static str, kind: &str) -> Self {
        // Test-only DDL from constants in this file.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE devagotchi.board_outbox ADD CONSTRAINT {name} CHECK(kind <> '{kind}')"
        )))
        .execute(fixture.test.store.admin())
        .await
        .unwrap();
        Self { fixture, name }
    }

    async fn lift(self) {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE devagotchi.board_outbox DROP CONSTRAINT {}",
            self.name
        )))
        .execute(self.fixture.test.store.admin())
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_a_standing_grant_covers_is_made_and_decided_together_or_not_at_all() {
    let f = or_skip!(fixture());
    let person = Actor::human("jesse");
    let agent = Actor::agent("claude");
    let grant = f
        .board
        .grant_standing(&person, "restart", None, WHAT, 7)
        .expect("grant");
    let chain = f.board.verify_chain().expect("chain");
    let refused = Refuse::new(&f, "refuse_decided", actions::APPROVAL_DECIDED).await;
    assert!(f.board.ask(&agent, &ask(Some("restart"))).is_err());
    refused.lift().await;
    // Neither the request nor its decision landed.
    assert_eq!(f.board.verify_chain().expect("chain"), chain);
    assert!(f.board.pending_approvals().expect("pending").is_empty());

    let (decided, by) = f.board.ask(&agent, &ask(Some("restart"))).expect("asked");
    assert_eq!(decided.status, "approved");
    assert_eq!(
        decided.decided_by.as_deref(),
        Some(&*format!("standing:{}", grant.id))
    );
    assert_eq!(by.map(|s| s.id), Some(grant.id));
    let actions: Vec<String> = f
        .board
        .events(chain.entries as i64, 100)
        .expect("events")
        .into_iter()
        .map(|e| e.action)
        .collect();
    assert_eq!(
        actions,
        [actions::APPROVAL_REQUESTED, actions::APPROVAL_DECIDED]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn approve_and_allow_decides_and_grants_together_or_not_at_all() {
    let f = or_skip!(fixture());
    let person = Actor::human("jesse");
    let agent = Actor::agent("claude");

    let (asked, _) = f.board.ask(&agent, &ask(Some("restart"))).expect("ask");
    let before = f.board.verify_chain().expect("chain").entries;
    let (decided, standing) = f
        .board
        .decide_and_allow(&person, &asked.id, asked.version, "fine", 7)
        .expect("approve and allow");
    assert_eq!(decided.status, "approved");
    let standing = standing.expect("a standing grant");
    assert_eq!(
        (standing.kind.as_str(), standing.what.as_str()),
        ("restart", WHAT)
    );
    assert_eq!(standing.expires_at - standing.granted_at, 7 * 86_400_000);
    let events = f.board.events(before as i64, 100).expect("events");
    let names: Vec<&str> = events.iter().map(|e| e.action.as_str()).collect();
    assert_eq!(
        names,
        [
            actions::APPROVAL_DECIDED,
            actions::APPROVAL_STANDING_GRANTED
        ]
    );
    assert_eq!(f.board.live_standing().expect("live"), vec![standing]);

    // An approval with no kind is approved and allows nothing.
    let (plain, _) = f.board.ask(&agent, &ask(None)).expect("plain");
    let (plain, none) = f
        .board
        .decide_and_allow(&person, &plain.id, plain.version, "", 7)
        .expect("plain");
    assert_eq!((plain.status.as_str(), none), ("approved", None));

    // At the limit, the grant is refused and the decision with it.
    for n in 1..MAX_LIVE {
        f.board
            .grant_standing(&person, "deploy", None, &format!("deploy {n}"), 1)
            .expect("grant");
    }
    let (full, _) = f.board.ask(&agent, &ask(Some("rotate"))).expect("ask");
    let refused = f
        .board
        .decide_and_allow(&person, &full.id, full.version, "", 7)
        .expect_err("too many");
    assert_eq!(refused.code, "too_many");
    let status = f.board.approval_status(&full.id).expect("status");
    assert_eq!(
        (status.status.as_str(), status.version),
        ("pending", full.version)
    );

    // Only a person, and only for 1 to 90 days.
    assert_eq!(
        f.board
            .decide_and_allow(&agent, &full.id, full.version, "", 7)
            .unwrap_err()
            .code,
        "humans_only"
    );
    assert_eq!(
        f.board
            .decide_and_allow(&person, &full.id, full.version, "", 91)
            .unwrap_err()
            .code,
        "invalid"
    );
    assert!(f.board.verify_chain().expect("chain").ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_takes_back_what_they_allowed_for_kinds_that_now_ask() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let ana = Actor::human("ana");
    let mine = f
        .board
        .grant_standing(&jesse, "restart", None, WHAT, 7)
        .expect("mine");
    let other_kind = f
        .board
        .grant_standing(&jesse, "deploy", None, "deploy web", 7)
        .expect("other kind");
    let theirs = f
        .board
        .grant_standing(&ana, "restart", None, "systemctl restart db", 7)
        .expect("theirs");
    let ended = f
        .board
        .revoke_own(&jesse, &["restart".to_string()])
        .expect("revoke own");
    assert_eq!(
        ended.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        [mine.id.as_str()]
    );
    assert_eq!(ended[0].revoked_by.as_deref(), Some("jesse"));
    let live: Vec<String> = f
        .board
        .live_standing()
        .expect("live")
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(live, [theirs.id, other_kind.id]);
    // Nothing left to take back: nothing written.
    let chain = f.board.verify_chain().expect("chain");
    let mut listener = board_changes(&f.test.url).await;
    assert!(
        f.board
            .revoke_own(&jesse, &["restart".to_string()])
            .expect("again")
            .is_empty()
    );
    assert_eq!(f.board.verify_chain().expect("chain"), chain);
    assert!(quiet(&mut listener).await);
    assert_eq!(
        f.board
            .revoke_own(&Actor::agent("claude"), &["deploy".to_string()])
            .unwrap_err()
            .code,
        "humans_only"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn delegating_assigns_and_says_why_in_one_event_or_not_at_all() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let id = f.new_ticket(&jesse, "port the scanner");
    let version = f.board.get(&id).expect("view").ticket.version;
    assert_eq!(
        f.board
            .delegate(&jesse, &id, version, "Not A Name", "yours")
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(
        f.board
            .delegate(&jesse, &id, version + 1, "codex", "yours")
            .unwrap_err()
            .code,
        "version_conflict"
    );
    let chain = f.board.verify_chain().expect("chain");
    let refused = Refuse::new(&f, "refuse_update", actions::TICKET_UPDATED).await;
    assert!(
        f.board
            .delegate(&jesse, &id, version, "codex", "yours")
            .is_err()
    );
    refused.lift().await;
    assert_eq!(f.board.verify_chain().expect("chain"), chain);
    let view = f.board.get(&id).expect("view");
    assert_eq!((view.ticket.assignee, view.notes.len()), (None, 0));

    let ticket = f
        .board
        .delegate(
            &jesse,
            &id,
            version,
            "codex",
            "yours: the Go scanner is the model",
        )
        .expect("delegate");
    assert_eq!(ticket.assignee.as_deref(), Some("codex"));
    assert_eq!(ticket.version, version + 1);
    let view = f.board.get(&id).expect("view");
    assert_eq!(view.notes.len(), 1);
    assert_eq!(view.notes[0].to_principal.as_deref(), Some("codex"));
    let events = f.board.events(chain.entries as i64, 100).expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, actions::TICKET_UPDATED);
    assert_eq!(
        events[0].data,
        serde_json::json!({"assignee": "codex", "note_seq": view.notes[0].seq})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn undo_reopens_a_ticket_only_within_the_window_on_the_database_clock() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let id = f.new_ticket(&jesse, "wrong button");
    let ticket = f.board.get(&id).expect("view").ticket;
    let closed = f
        .board
        .update(
            &jesse,
            &id,
            ticket.version,
            Patch {
                state: Some("cancelled".into()),
                ..Patch::default()
            },
        )
        .expect("close");
    assert!(closed.closed_at.is_some());
    let reopened = f
        .board
        .undo_close(&jesse, &id, closed.version, "ready")
        .expect("undo");
    assert_eq!(reopened.state, "ready");
    assert_eq!(reopened.closed_at, None);
    let last = f.board.events(0, 100).expect("events").pop().unwrap();
    assert_eq!(last.action, actions::TICKET_UPDATED);
    assert_eq!(
        last.data,
        serde_json::json!({"from": "cancelled", "to": "ready", "undo": true})
    );
    // Not closed: nothing to undo.
    assert_eq!(
        f.board
            .undo_close(&jesse, &id, reopened.version, "ready")
            .unwrap_err()
            .code,
        "undo_too_late"
    );
    // Closed again, and the window passes.
    let closed = f
        .board
        .update(
            &jesse,
            &id,
            reopened.version,
            Patch {
                state: Some("cancelled".into()),
                ..Patch::default()
            },
        )
        .expect("close again");
    f.advance(crate::UNDO_MS + 1);
    assert_eq!(
        f.board
            .undo_close(&jesse, &id, closed.version, "ready")
            .unwrap_err()
            .code,
        "undo_too_late"
    );
    assert_eq!(f.board.get(&id).expect("view").ticket.state, "cancelled");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_webhook_is_set_and_taken_away_in_one_transaction() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let secret = f
        .board
        .set_wake(&jesse, "hermes", Some("https://example.com/hook"))
        .expect("set")
        .expect("a secret");
    assert_eq!(secret.len(), 64);
    let wakes = f.board.wakes().expect("wakes");
    assert_eq!(wakes["hermes"].url, "https://example.com/hook");
    assert_eq!(wakes["hermes"].secret, secret);
    let last = f.board.events(0, 100).expect("events").pop().unwrap();
    assert_eq!(last.action, actions::WAKE_SET);
    assert_eq!(
        last.data,
        serde_json::json!({"recipient": "hermes", "url": "https://example.com/hook"})
    );
    assert!(
        !last.data.to_string().contains(&secret),
        "the secret stays off the chain"
    );
    assert_eq!(f.board.set_wake(&jesse, "hermes", None).expect("off"), None);
    assert!(f.board.wakes().expect("wakes").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_made_without_a_repo_takes_the_boards() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let bare = f.new_ticket(&jesse, "before a repo");
    assert_eq!(f.board.get(&bare).expect("view").ticket.repo, None);
    f.board
        .set_project(
            &jesse,
            &ProjectSettings {
                repo: Some("https://github.com/you/project".into()),
                environments: None,
            },
        )
        .expect("repo");
    let defaulted = f.new_ticket(&jesse, "after");
    assert_eq!(
        f.board
            .get(&defaulted)
            .expect("view")
            .ticket
            .repo
            .as_deref(),
        Some("https://github.com/you/project")
    );
    let mut named = NewTicket {
        title: "elsewhere".into(),
        ..NewTicket::default()
    };
    named.anchors.repo = Some("https://github.com/you/other".into());
    let named = f.board.create(&jesse, named).expect("named");
    assert_eq!(named.repo.as_deref(), Some("https://github.com/you/other"));
}
