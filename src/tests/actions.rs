//! What the chain says (`.ai/patterns/board-operations.md`, rules 6 and 7): every action is
//! `<subject>.<past participle>` from `chain::actions`, a request that changes nothing appends
//! nothing and announces nothing, and an event's data names what changed under its own keys.

use std::path::Path;

use super::{board_changes, fixture, or_skip, quiet};
use crate::{Actor, DepKind, NewTicket, Patch, ProjectSettings, Settings, TicketSource, actions};

const SUBJECTS: [&str; 8] = [
    "ticket", "lease", "note", "dep", "approval", "board", "issue", "wake",
];

#[test]
fn every_action_is_a_subject_and_a_past_participle() {
    let mut seen = std::collections::BTreeSet::new();
    for action in actions::ALL {
        let (subject, verb) = action
            .split_once('.')
            .unwrap_or_else(|| panic!("{action}: no subject"));
        assert!(SUBJECTS.contains(&subject), "{action}: subject {subject}");
        assert!(
            !verb.is_empty() && verb.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
            "{action}: {verb} is not [a-z_]+"
        );
        // One word is the participle: `settings_changed`, `handed_off`, `set`.
        assert!(
            verb.split('_')
                .any(|word| word.ends_with("ed") || word.ends_with("en") || word == "set"),
            "{action}: {verb} has no past participle"
        );
        assert!(seen.insert(action), "{action} twice");
    }
}

/// The text of each call to `event` or `clear_lease` in the store, from the call to its
/// `.await`, whitespace removed.
fn appends(source: &str) -> Vec<String> {
    let flat: String = source.split_whitespace().collect();
    let mut found = Vec::new();
    for marker in ["self.event(", "self.clear_lease("] {
        for piece in flat.split(marker).skip(1) {
            found.push(piece.split(".await").next().unwrap_or(piece).to_string());
        }
    }
    found
}

/// A `"word.word"` literal: what an action name passed by hand looks like.
fn has_action_literal(call: &str) -> bool {
    call.split('"').skip(1).step_by(2).any(|text| {
        text.split_once('.').is_some_and(|(a, b)| {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes().all(|c| c.is_ascii_lowercase())
                && b.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
        })
    })
}

#[test]
fn the_store_names_every_action_by_its_constant() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<_> = std::fs::read_dir(src.join("hosted"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.push(src.join("approval.rs"));
    let mut calls = 0;
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        for call in appends(&text) {
            calls += 1;
            assert!(
                !has_action_literal(&call),
                "{}: an action passed as a literal: {call}",
                path.display()
            );
        }
    }
    assert!(calls >= 20, "the scan found only {calls} appends");
    // The scan sees what it looks for.
    assert!(has_action_literal(
        r#"connection,actor.id,now,"ticket.heartbeat",Some(id)"#
    ));
    assert!(!has_action_literal(
        r#"connection,actor.id,now,actions::LEASE_EXTENDED,Some(id)"#
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_that_changes_nothing_appends_and_announces_nothing() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let claude = Actor::agent("claude");
    let a = f.new_ticket(&jesse, "a");
    let b = f.new_ticket(&jesse, "b");
    f.board
        .add_dep(&jesse, &a, &b, DepKind::Blocks)
        .expect("dep");
    f.board
        .set_project(
            &jesse,
            &ProjectSettings {
                repo: Some("https://github.com/you/project".into()),
                environments: Some(vec!["dev".into(), "prod".into()]),
            },
        )
        .expect("settings");
    let source = TicketSource {
        path: "plans/one.md".into(),
        item: "m1".into(),
        revision: None,
        content_sha256: None,
    };
    let sourced = NewTicket {
        title: "from a plan".into(),
        source: Some(source),
        ..NewTicket::default()
    };
    let made = f.board.create(&jesse, sourced.clone()).expect("sourced");
    let held = f.new_ticket(&jesse, "held");
    let lease = f.board.claim(&claude, &held, None).expect("claim");
    let settings = f.board.settings().expect("settings");
    let workflow = f.board.workflow().expect("workflow");
    let version = f.board.get(&a).expect("a").ticket.version;
    let chain = f.board.verify_chain().expect("chain");
    let mut listener = board_changes(&f.test.url).await;

    // The same dependency again: the graph as it is.
    let deps = f
        .board
        .add_dep(&jesse, &a, &b, DepKind::Blocks)
        .expect("again");
    assert_eq!(deps.len(), 1);
    // An empty patch: the ticket as it is.
    let same = f
        .board
        .update(&jesse, &a, version, Patch::default())
        .expect("empty patch");
    assert_eq!(same.version, version);
    // The project's settings as they already are, field by field and together.
    for patch in [
        ProjectSettings {
            repo: Some("https://github.com/you/project".into()),
            environments: None,
        },
        ProjectSettings {
            repo: None,
            environments: Some(vec!["dev".into(), "prod".into()]),
        },
        ProjectSettings::default(),
    ] {
        assert_eq!(f.board.set_project(&jesse, &patch).expect("same"), settings);
    }
    // Settings and workflow replaced by themselves.
    f.board
        .put_settings(&jesse, &settings)
        .expect("same settings");
    f.board
        .put_workflow(&jesse, &workflow)
        .expect("same workflow");
    // Re-claiming what you hold: the lease you have.
    let again = f.board.claim(&claude, &held, None).expect("re-claim");
    assert!(again.already_held);
    assert_eq!(again.fence, lease.fence);
    // The same origin with the same inputs: the ticket it made.
    assert_eq!(f.board.create(&jesse, sourced).expect("repeat").id, made.id);
    // A webhook taken away that was never there.
    assert_eq!(
        f.board.set_wake(&jesse, "nobody", None).expect("no wake"),
        None
    );

    assert_eq!(f.board.verify_chain().expect("chain"), chain);
    assert!(quiet(&mut listener).await, "a no-op announced a change");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_names_only_what_changed() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let mut settings = f.board.settings().expect("settings");
    settings.wip_limit_default = 3;
    f.board.put_settings(&jesse, &settings).expect("put");
    let events = f.board.events(0, 1000).expect("events");
    let last = events.last().expect("an event");
    assert_eq!(last.action, actions::BOARD_SETTINGS_REPLACED);
    assert_eq!(
        last.data,
        serde_json::json!({"changed": ["wip_limit_default"]})
    );
    assert_eq!(f.board.settings().expect("settings"), settings);
    assert_ne!(settings, Settings::default());

    f.board
        .set_project(
            &jesse,
            &ProjectSettings {
                repo: Some("https://github.com/you/project".into()),
                environments: Some(Settings::default().environments),
            },
        )
        .expect("repo only differs");
    let last = f.board.events(0, 1000).expect("events").pop().unwrap();
    assert_eq!(last.action, actions::BOARD_SETTINGS_CHANGED);
    assert_eq!(
        last.data,
        serde_json::json!({"repo": "https://github.com/you/project"})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handoff_records_its_recipient_and_where_the_ticket_went() {
    let f = or_skip!(fixture());
    let jesse = Actor::human("jesse");
    let claude = Actor::agent("claude");
    let id = f.new_ticket(&jesse, "cross-vendor");
    let lease = f.board.claim(&claude, &id, None).expect("claim");
    f.board
        .handoff(&claude, &id, lease.fence, "over to you", Some("codex"))
        .expect("handoff");
    let last = f.board.events(0, 1000).expect("events").pop().unwrap();
    assert_eq!(last.action, actions::TICKET_HANDED_OFF);
    assert_eq!(last.data["assignee"], "codex");
    assert_eq!(last.data["from"], lease.ticket.state);
    assert_eq!(last.data["to"], "ready");
    assert_eq!(last.fence, Some(lease.fence));
    assert!(f.board.verify_chain().expect("chain").ok);
}
