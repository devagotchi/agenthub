//! What the command line prints and records: `prime`, `show`, the export, anchors, a session.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn prime_text_stays_short_and_names_the_protocol() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            run(dir.path(), None, false, &["init", "--project", "demo"]).0,
            EXIT_OK
        );
        for n in 0..30 {
            let title = format!("ticket {n}");
            assert_eq!(run(dir.path(), None, false, &["new", &title]).0, EXIT_OK);
        }
        let (code, out, _) = run(dir.path(), Some("claude"), true, &["prime"]);
        assert_eq!(code, EXIT_OK);
        assert!(out.lines().count() <= 50, "{} lines", out.lines().count());
        assert!(out.contains("protocol: claim → beat → note → done"));
        assert!(out.starts_with("board demo · you are claude"));
        let (code, out, _) = run(dir.path(), Some("claude"), true, &["prime", "--quiet"]);
        assert_eq!(code, EXIT_OK);
        assert!(out.is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn export_writes_one_json_line_per_ticket_and_verify_reads_the_chain() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            run(dir.path(), None, false, &["init", "--project", "demo"]).0,
            EXIT_OK
        );
        assert_eq!(run(dir.path(), None, false, &["new", "a"]).0, EXIT_OK);
        assert_eq!(run(dir.path(), None, false, &["new", "b"]).0, EXIT_OK);
        let (code, out, _) = run(dir.path(), None, false, &["export"]);
        assert_eq!(code, EXIT_OK, "{out}");
        let text = std::fs::read_to_string(dir.path().join(".devagotchi/board.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 2);
        let (code, out, _) = run(dir.path(), None, false, &["verify"]);
        assert_eq!(code, EXIT_OK);
        assert_eq!(json(&out)["entries"], 2);
        assert_eq!(json(&out)["ok"], true);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn prime_and_show_print_the_anchors_as_one_line() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(
            run(d, None, false, &["init", "--project", "demo"]).0,
            EXIT_OK
        );
        let (_, out, _) = run(
            d,
            Some("claude"),
            false,
            &[
                "new",
                "dabo is down",
                "--machine",
                "dabo",
                "--env",
                "prod",
                "--repo",
                "https://github.com/x/y",
                "--agent",
                "claude",
                "--session",
                "4f1c9a2b-77e0",
            ],
        );
        let id = json(&out)["id"].as_str().unwrap().to_string();
        let line = "dabo · prod · github.com/x/y · claude (session 4f1c9a2b…)";
        let (code, shown, _) = run(d, Some("claude"), true, &["show", &id]);
        assert_eq!(code, EXIT_OK, "{shown}");
        assert!(shown.lines().any(|l| l.trim() == line), "{shown}");
        let (code, primed, _) = run(d, Some("claude"), true, &["prime"]);
        assert_eq!(code, EXIT_OK, "{primed}");
        let row = primed.lines().find(|l| l.contains(&id)).expect("ready row");
        assert!(row.ends_with(&format!("· {line}")), "{row}");
        let (_, primed, _) = run(d, Some("claude"), false, &["prime"]);
        assert_eq!(json(&primed)["ready"][0]["anchors"], line);
        let (_, plain, _) = run(d, Some("codex"), false, &["new", "plain"]);
        let plain_id = json(&plain)["id"].as_str().unwrap().to_string();
        let (_, shown, _) = run(d, Some("claude"), true, &["show", &plain_id]);
        assert_eq!(shown.lines().count(), 2, "nothing known, no line: {shown}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_ticket_carries_the_session_the_hook_pack_exported() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            run(dir.path(), None, false, &["init", "--project", "demo"]).0,
            EXIT_OK
        );
        let new = |session: Option<&str>, args: &[&str]| {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = cli::run(
                Invocation {
                    args: args.iter().map(OsString::from).collect(),
                    cwd: dir.path().to_path_buf(),
                    actor_env: Some("claude".into()),
                    session_env: session.map(str::to_string),
                    os_user: Some("jesse".into()),
                    stdout_is_terminal: false,
                    account_file: None,
                    plugin_host: None,
                    storage_env: storage(),
                    hub: None,
                },
                &mut out,
                &mut err,
            );
            assert_eq!(code, EXIT_OK, "{}", String::from_utf8_lossy(&err));
            json(&String::from_utf8(out).unwrap())
        };
        assert_eq!(
            new(Some("s-env"), &["new", "from the hook"])["session"],
            "s-env"
        );
        assert_eq!(
            new(Some("s-env"), &["new", "flag wins", "--session", "s-flag"])["session"],
            "s-flag"
        );
        assert!(new(None, &["new", "none"])["session"].is_null());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn anchors_are_flags_on_new_set_and_ls_and_in_the_json() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(
            run(d, None, false, &["init", "--project", "demo"]).0,
            EXIT_OK
        );
        let (code, out, _) = run(
            d,
            None,
            false,
            &[
                "new",
                "dabo is down",
                "--machine",
                "dabo",
                "--repo",
                "https://github.com/x/y",
                "--env",
                "prod",
                "--agent",
                "claude",
                "--session",
                "s-1",
                "--fingerprint",
                "fp-1",
            ],
        );
        assert_eq!(code, EXIT_OK, "{out}");
        let ticket = json(&out);
        for (key, value) in [
            ("machine", "dabo"),
            ("repo", "https://github.com/x/y"),
            ("environment", "prod"),
            ("agent", "claude"),
            ("session", "s-1"),
            ("fingerprint", "fp-1"),
        ] {
            assert_eq!(ticket[key], value, "{key}: {ticket}");
        }
        assert_eq!(ticket["occurrences"], 1);
        let id = ticket["id"].as_str().unwrap().to_string();
        let (_, plain, _) = run(d, None, false, &["new", "plain"]);
        let plain = json(&plain);
        assert!(
            plain["machine"].is_null() && plain.get("machine").is_some(),
            "{plain}"
        );

        let ids = |args: &[&str]| -> Vec<String> {
            let (code, out, _) = run(d, None, false, args);
            assert_eq!(code, EXIT_OK, "{out}");
            json(&out)
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["id"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            ids(&["ls", "--machine", "dabo", "--env", "prod"]),
            [id.clone()]
        );
        assert_eq!(ids(&["ls", "--fingerprint", "fp-1"]), [id.clone()]);
        assert!(ids(&["ls", "--machine", "other"]).is_empty());
        assert_eq!(ids(&["ls"]).len(), 2);

        let (code, out, _) = run(
            d,
            None,
            false,
            &[
                "set",
                &id,
                "--version",
                "1",
                "--env",
                "staging",
                "--session",
                "",
            ],
        );
        assert_eq!(code, EXIT_OK, "{out}");
        assert_eq!(json(&out)["environment"], "staging");
        assert!(json(&out)["session"].is_null(), "an empty anchor clears it");
        assert_eq!(ids(&["ls", "--env", "staging"]), [id.clone()]);

        let (code, out, _) = run(d, None, false, &["new", "qa only", "--env", "qa"]);
        assert_ne!(code, EXIT_OK);
        let error = &json(&out)["error"];
        assert_eq!(error["code"], "unknown_environment");
        assert!(
            error["next"]
                .as_str()
                .unwrap()
                .contains("dev, staging, prod"),
            "{error}"
        );
    })
    .await;
}

/// Store codes no command can act on: runtime plumbing, or reached only by a hub page or
/// the webhook route.
const INTERNAL: &[&str] = &[
    "runtime",
    "board_random",
    "board_id",
    "board_runtime",
    "delivery_conflict",
    "unsupported_action",
    "wake_failed",
    "no_done_state",
    "too_many",
    "undo_too_late",
    // The CLI has no approval verbs (`Face` has none), so these never reach it.
    "not_an_approval",
    "approval_changed",
    "already_decided",
];

/// Every refusal code in the files where the store decides.
fn store_codes() -> std::collections::BTreeSet<String> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<_> = [
        "approval.rs",
        "lease.rs",
        "wake.rs",
        "issue_lifecycle.rs",
        "settings.rs",
        "workflow.rs",
        "checks.rs",
        "lib.rs",
    ]
    .iter()
    .map(|f| src.join(f))
    .collect();
    fn collect(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    for dir in ["hosted", "board"] {
        collect(&src.join(dir), &mut files);
    }
    let mut codes = std::collections::BTreeSet::new();
    for path in files {
        let flat: String = std::fs::read_to_string(&path)
            .unwrap()
            .split_whitespace()
            .collect();
        for marker in ["fail(\"", "BridgeError::new(\""] {
            for piece in flat.split(marker).skip(1) {
                codes.insert(piece.split('"').next().unwrap().to_string());
            }
        }
    }
    codes
}

#[test]
fn every_code_a_command_can_reach_names_its_next_step() {
    let mut codes = store_codes();
    assert!(codes.len() > 20, "the scan found {} codes", codes.len());
    // `invalid()` builds `invalid`; the hub refuses a viewer with `read_only`; the rest are
    // the command line's own.
    for code in [
        "invalid",
        "read_only",
        "no_storage",
        "invalid_storage",
        "not_signed_in",
        "hub_unreachable",
    ] {
        codes.insert(code.to_string());
    }
    let fallback = crate::cli::errors::next_for("zz_never_a_code", None);
    let missing: Vec<&String> = codes
        .iter()
        .filter(|code| !INTERNAL.contains(&code.as_str()))
        .filter(|code| crate::cli::errors::next_for(code, Some("x")) == fallback)
        .collect();
    assert!(
        missing.is_empty(),
        "no next of its own in cli/errors.rs: {missing:?}"
    );
}

#[test]
fn a_refusal_against_the_boards_state_is_a_conflict() {
    for code in [
        "parent_closed",
        "request_conflict",
        "dependency_cycle",
        "not_held",
        "source_conflict",
        "version_conflict",
    ] {
        assert_eq!(
            crate::cli::errors::exit_code_for(code),
            EXIT_CONFLICT,
            "{code}"
        );
    }
}
