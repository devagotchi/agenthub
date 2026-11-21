//! October hint.go, notes.go and preset settings: behavior changes through stored settings.

use super::*;
use crate::SettingsEdit;

#[tokio::test(flavor = "multi_thread")]
async fn board_runtime_settings_control_heartbeat_hints_and_note_kinds() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let worker = Actor::agent("worker");
    let state = f.board.configuration_state().unwrap();
    let mut edit = SettingsEdit::from(&state.settings);
    edit.heartbeat_hint_ms = Some(5_000);
    edit.note_kinds = Some(vec!["note".into(), "triage".into()]);
    f.board
        .edit_settings(&human, state.settings_version, &edit)
        .unwrap();
    let id = f.new_ticket(&human, "work with configured notes");
    let claimed = f.board.claim(&worker, &id, Some(30_000)).unwrap();
    assert_eq!(claimed.heartbeat_every_ms, 5_000);
    assert_eq!(
        f.board.prime(&worker, 20).unwrap().heartbeat_every_ms,
        5_000
    );
    let beat = f
        .board
        .heartbeat(&worker, &id, claimed.fence, None)
        .unwrap();
    assert_eq!(beat.heartbeat_every_ms, 5_000);
    let events = f.board.events(0, 100).unwrap();
    let error = f
        .board
        .add_note(
            &worker,
            &id,
            "unknown",
            "not permitted",
            None,
            Some(claimed.fence),
        )
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert_eq!(f.board.events(0, 100).unwrap(), events);
    f.board
        .add_note(
            &worker,
            &id,
            "triage",
            "investigated",
            None,
            Some(claimed.fence),
        )
        .unwrap();
    // Lease lifecycle records cannot be disabled by removing their kinds from the UI list.
    f.board
        .complete(&worker, &id, claimed.fence, "actual work")
        .unwrap();
    assert!(
        f.board
            .get(&id)
            .unwrap()
            .notes
            .iter()
            .any(|note| note.kind == "summary")
    );
    let state = f.board.configuration_state().unwrap();
    let mut edit = SettingsEdit::from(&state.settings);
    edit.heartbeat_hint_ms = Some(90_000);
    edit.note_kinds = Some(Vec::new());
    f.board
        .edit_settings(&human, state.settings_version, &edit)
        .unwrap();
    let next_id = f.new_ticket(&human, "shorter lease");
    let short = f.board.claim(&worker, &next_id, Some(30_000)).unwrap();
    assert_eq!(short.heartbeat_every_ms, 10_000, "actual TTL caps the hint");
    f.board
        .add_note(
            &human,
            &next_id,
            "custom",
            "allowed with empty list",
            None,
            None,
        )
        .unwrap();
    assert!(f.board.verify_chain().unwrap().ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn board_runtime_settings_reject_invalid_values_without_writes() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let initial = f.board.configuration_state().unwrap();
    let events = f.board.events(0, 100).unwrap();
    let mut bad = SettingsEdit::from(&initial.settings);
    for hint in [0, -1, 999] {
        bad.heartbeat_hint_ms = Some(hint);
        assert!(
            f.board
                .edit_settings(&human, initial.settings_version, &bad)
                .is_err()
        );
    }
    bad.heartbeat_hint_ms = Some(5_000);
    for kinds in [vec!["note".into(), "note".into()], vec!["bad type".into()]] {
        bad.note_kinds = Some(kinds);
        assert!(
            f.board
                .edit_settings(&human, initial.settings_version, &bad)
                .is_err()
        );
    }
    assert_eq!(f.board.configuration_state().unwrap(), initial);
    assert_eq!(f.board.events(0, 100).unwrap(), events);
    // Existing stored settings need no rewrite and retain unrestricted historical notes.
    let mut old = serde_json::to_value(&initial.settings).unwrap();
    old.as_object_mut().unwrap().remove("heartbeat_hint_ms");
    old.as_object_mut().unwrap().remove("note_kinds");
    let old: Settings = serde_json::from_value(old).unwrap();
    assert_eq!(old.heartbeat_every_ms(900_000), 300_000);
    assert!(old.check_note_kind("old-custom-type").is_ok());
}
