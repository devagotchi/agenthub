//! `board new --origin` on a local board: an imported ticket worked through its lanes while
//! its importer repeats itself, a conflicting import refused with exit 2, the origin in show
//! and export.

use super::*;

const ORIGIN: &str = r#"{"path":"docs/ROADMAP.md","item":"scanner-preview","revision":"commit:0123","content_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

#[tokio::test(flavor = "multi_thread")]
async fn an_imported_ticket_is_made_once_and_worked_through_its_lanes() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ok = |actor: Option<&str>, args: &[&str]| {
            let (code, out, err) = run(root, actor, false, args);
            assert_eq!(code, EXIT_OK, "{args:?}: {out}{err}");
            json(&out)
        };
        ok(None, &["init", "--project", "demo"]);
        let import = ["new", "Implement project scanner", "--origin", ORIGIN];
        let made = ok(Some("importer"), &import);
        let id = made["id"].as_str().unwrap().to_string();

        // The importer runs again while a worker holds it: the same ticket, lease untouched.
        let lease = ok(Some("worker"), &["claim", &id]);
        let again = ok(Some("importer"), &import);
        assert_eq!(again["id"], id.as_str());
        assert_eq!(again["holder"], "worker");
        assert_eq!(again["fence"], lease["fence"]);
        ok(Some("worker"), &["done", &id, "built"]);
        let (code, out, _) = run(root, Some("worker"), false, &["claim", &id]);
        assert_eq!(code, EXIT_CONFLICT, "the author does not review: {out}");
        assert_eq!(json(&out)["error"]["code"], "distinct_required");
        ok(Some("reviewer"), &["claim", &id]);
        let done = ok(Some("reviewer"), &["done", &id, "reviewed"]);
        assert_eq!(done["state"], "done", "{done}");

        // After it is done, a repeat still returns it, done; a changed import is refused.
        let after = ok(Some("importer"), &import);
        assert_eq!(
            (&after["id"], &after["state"]),
            (&made["id"], &done["state"])
        );
        let events = ok(None, &["verify"])["entries"].clone();
        let changed = ["new", "A different scope", "--origin", ORIGIN];
        let (code, out, _) = run(root, Some("importer"), false, &changed);
        assert_eq!(code, EXIT_CONFLICT, "{out}");
        let refused = json(&out);
        assert_eq!(refused["error"]["code"], "source_conflict");
        assert!(
            refused["error"]["message"].as_str().unwrap().contains(&id),
            "{refused}"
        );
        assert!(
            refused["error"]["next"]
                .as_str()
                .unwrap()
                .contains("reconcile"),
            "{refused}"
        );
        assert_eq!(ok(None, &["verify"])["entries"], events, "nothing written");
        let (code, out, _) = run(root, None, false, &["new", "x", "--origin", "{\"path\":1}"]);
        assert_eq!(code, EXIT_USAGE, "{out}");

        // show and export carry the origin, and who recorded it.
        let shown = ok(None, &["show", &id]);
        assert_eq!(shown["provenance"]["source"]["item"], "scanner-preview");
        assert_eq!(shown["provenance"]["recorded_by"], "importer");
        let (_, text, _) = run(root, None, true, &["show", &id]);
        assert!(
            text.contains("origin docs/ROADMAP.md#scanner-preview · revision commit:0123"),
            "{text}"
        );
        let at = shown["provenance"]["recorded_at"].as_i64().unwrap();
        assert!(
            text.contains(&format!("recorded by importer at {}", cli::utc(at))),
            "{text}"
        );
        let out = root.join("board.jsonl");
        ok(None, &["export", "--out", out.to_str().unwrap()]);
        let exported = std::fs::read_to_string(&out).unwrap();
        let row: serde_json::Value = exported
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .find(|row: &serde_json::Value| row["ticket"]["id"] == id.as_str())
            .unwrap();
        assert_eq!(row["provenance"], shown["provenance"]);
        assert_eq!(ok(None, &["verify"])["ok"], true);
        assert_eq!(ok(None, &["ls"]).as_array().map(Vec::len), Some(1));
    })
    .await;
}

/// `board show` prints a board time as UTC, leap days and times before 1970 included.
#[test]
fn board_times_print_as_utc() {
    for (ms, text) in [
        (0, "1970-01-01T00:00:00Z"),
        (-1, "1969-12-31T23:59:59Z"),
        (951_782_400_000, "2000-02-29T00:00:00Z"),
        (1_795_021_509_000, "2026-11-18T17:05:09Z"),
        (4_107_628_799_000, "2100-03-01T23:59:59Z"),
    ] {
        assert_eq!(cli::utc(ms), text, "{ms}");
    }
}
