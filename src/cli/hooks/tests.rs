use super::*;
use crate::NewTicket;

fn invoke(board: &dyn Face, phase: &str, body: &str, env: Option<&Path>) -> (u8, String, String) {
    let input = read_input(body.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run(
        board,
        &Actor::agent("claude"),
        phase,
        &input,
        env,
        &mut Output {
            json: false,
            quiet: false,
            out: &mut out,
            err: &mut err,
        },
    );
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn input_is_typed_bounded_and_never_shell_parsed() {
    for bad in [
        "not JSON",
        r#"{"hook_event_name":"SessionStart","stop_hook_active":"true"}"#,
        r#"{"hook_event_name":"SessionStart","session_id":"x; rm -rf /"}"#,
        r#"{"hook_event_name":"SessionStart","cwd":"relative"}"#,
    ] {
        assert!(read_input(bad.as_bytes()).is_err(), "{bad}");
    }
    assert!(read_input(vec![b' '; MAX_INPUT + 1].as_slice()).is_err());
    assert!(
        read_input(
            br#"{"hook_event_name":"SessionStart","session_id":"4f1c-9a.b_2","source":"resume"}"#
                .as_slice()
        )
        .is_ok()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn native_hooks_preserve_context_throttle_beats_and_report_interruption_without_mutating_other_sessions()
 {
    let f = crate::tests::or_skip!(crate::tests::fixture());
    let dir = tempfile::tempdir().unwrap();
    let board = &f.board;
    let actor = Actor::agent("claude");
    let ticket = board
        .create(
            &actor,
            NewTicket {
                title: "Keep real evidence".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let held = board.claim(&actor, &ticket.id, None).unwrap();
    board
        .add_note(
            &actor,
            &ticket.id,
            "decision",
            "Previously verified evidence",
            None,
            Some(held.fence),
        )
        .unwrap();
    let env = dir.path().join("environment");
    let (code, out, err) = invoke(
        board,
        "session-start",
        r#"{"hook_event_name":"SessionStart","session_id":"session-1","source":"resume"}"#,
        Some(&env),
    );
    assert_eq!(code, EXIT_OK, "{err}");
    let response: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        response["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    let context = response["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("Previously verified evidence"),
        "{context}"
    );
    assert_eq!(
        std::fs::read_to_string(&env).unwrap(),
        "export DEVAGOTCHI_SESSION=session-1\n"
    );
    let count = board.events(0, 100).unwrap().len();
    let (code, out, _) = invoke(board, "beat", r#"{"hook_event_name":"PostToolUse"}"#, None);
    assert_eq!(code, EXIT_OK);
    assert_eq!(out.trim(), "{}");
    assert_eq!(board.events(0, 100).unwrap().len(), count);
    f.advance(held.heartbeat_every_ms + 1);
    let (code, _, err) = invoke(board, "beat", r#"{"hook_event_name":"PostToolUse"}"#, None);
    assert_eq!(code, EXIT_OK, "{err}");
    assert!(
        board
            .get(&ticket.id)
            .unwrap()
            .ticket
            .lease_expires_at
            .unwrap()
            > held.lease_expires_at - held.heartbeat_every_ms - 1,
        "the beat extended the lease that was due"
    );
    assert_eq!(board.events(0, 100).unwrap().len(), count + 1);
    let (code, _, err) = invoke(
        board,
        "stop",
        r#"{"hook_event_name":"Stop","stop_hook_active":false}"#,
        None,
    );
    assert_eq!(code, EXIT_CONFLICT);
    assert!(err.contains(&ticket.id), "{err}");
    let (code, _, err) = invoke(
        board,
        "stop",
        r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
        None,
    );
    assert_eq!(code, EXIT_UNAVAILABLE);
    assert!(err.contains("No completion or automatic release"), "{err}");
    assert_eq!(
        board.get(&ticket.id).unwrap().ticket.holder.as_deref(),
        Some("claude")
    );
    let (code, _, err) = invoke(
        board,
        "session-end",
        r#"{"hook_event_name":"SessionEnd"}"#,
        None,
    );
    assert_eq!(code, EXIT_UNAVAILABLE, "{err}");
    board
        .handoff(
            &actor,
            &ticket.id,
            held.fence,
            "Continue with the recorded evidence",
            None,
        )
        .unwrap();
    let (code, out, err) = invoke(board, "stop", r#"{"hook_event_name":"Stop"}"#, None);
    assert_eq!(code, EXIT_OK, "{err}");
    assert_eq!(out.trim(), "{}");
    assert!(board.verify_chain().unwrap().ok);
    let (code, _, err) = invoke(board, "beat", r#"{"hook_event_name":"Stop"}"#, None);
    assert_eq!(code, EXIT_CONFLICT);
    assert!(err.contains("does not match"), "{err}");
}
