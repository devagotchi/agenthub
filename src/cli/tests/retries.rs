//! Separate invocations recover original inputs; replay never changes the newer lease.
use super::*;

fn ok(root: &Path, args: &[&str]) -> serde_json::Value {
    let (code, out, err) = run(root, Some("worker"), false, args);
    assert_eq!(code, EXIT_OK, "{args:?}: {out} {err}");
    assert!(err.is_empty(), "JSON keeps stderr empty: {err}");
    json(&out)
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_create_retry_restores_session_and_rejects_changed_intent() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        ok(root, &["init", "--project", "demo"]);
        let args = ["new", "one original task", "--request-id", "create-once"];
        let invoke = |session| {
            let (code, out, err) = run_session(root, Some("worker"), false, &args, Some(session));
            assert_eq!(code, EXIT_OK, "{out} {err}");
            json(&out)
        };
        let first = invoke("original-session");
        let retry = invoke("a-later-session");
        assert_eq!(first["request_id"], "create-once");
        assert_eq!(first["replayed"], false);
        assert_eq!(retry["replayed"], true);
        assert_eq!(retry["id"], first["id"]);
        assert_eq!(retry["session"], first["session"]);
        assert_eq!(first["session"], "original-session");
        assert_eq!(ok(root, &["verify"])["entries"], 1);
        for args in [
            vec!["new", "different task", "--request-id", "create-once"],
            vec![
                "claim",
                first["id"].as_str().unwrap(),
                "--request-id",
                "create-once",
            ],
        ] {
            let (code, out, _) = run(root, Some("worker"), false, &args);
            assert_eq!(code, EXIT_CONFLICT, "{out}");
            assert_eq!(json(&out)["error"]["code"], "request_conflict");
        }
        assert_eq!(ok(root, &["verify"])["entries"], 1);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_concurrent_retry_records_one_mutation_and_preparation_failure_records_none() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        ok(root, &["init", "--project", "demo"]);
        let database = storage();
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let invoke = || {
                STORAGE.set(database.clone());
                barrier.wait();
                ok(
                    root,
                    &["new", "concurrent task", "--request-id", "concurrent"],
                )
            };
            let first = scope.spawn(invoke);
            let second = scope.spawn(invoke);
            [first.join().unwrap(), second.join().unwrap()]
        });
        assert_eq!(results[0]["id"], results[1]["id"]);
        assert_ne!(results[0]["replayed"], results[1]["replayed"]);
        assert_eq!(ok(root, &["verify"])["entries"], 1);
        // A malformed saved record cannot fall through to a fresh server mutation.
        let requests = root.join(".devagotchi/requests");
        let record = std::fs::read_dir(requests)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "json"))
            .unwrap();
        std::fs::write(record, b"{torn").unwrap();
        let (code, out, _) = run(
            root,
            Some("worker"),
            false,
            &["new", "concurrent task", "--request-id", "concurrent"],
        );
        assert_eq!(code, EXIT_UNAVAILABLE, "{out}");
        assert_eq!(json(&out)["error"]["code"], "board_io");
        assert_eq!(ok(root, &["verify"])["entries"], 1);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_old_claim_and_release_replays_preserve_new_lease_and_cache() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        ok(root, &["init", "--project", "demo"]);
        let made = ok(root, &["new", "lease history"]);
        let id = made["id"].as_str().unwrap();
        let claim = ["claim", id, "--request-id", "claim-original"];
        let release = ["release", id, "--request-id", "release-original"];
        let original = ok(root, &claim);
        let released = ok(root, &release);
        let retried = ok(root, &release);
        assert_eq!(retried["replayed"], true, "cache was already removed");
        assert_eq!(retried["version"], released["version"]);
        let newer = ok(root, &["claim", id, "--request-id", "claim-newer"]);
        assert!(newer["fence"].as_i64().unwrap() > original["fence"].as_i64().unwrap());
        let cache = cli::claims::cache_path(&root.join(".devagotchi"), "worker");
        let before = std::fs::read(&cache).unwrap();
        assert_eq!(ok(root, &release)["replayed"], true);
        let replay = ok(root, &claim);
        assert_eq!(replay["fence"], original["fence"]);
        assert_eq!(replay["lease_expires_at"], original["lease_expires_at"]);
        assert_eq!(replay["replayed"], true);
        assert_eq!(std::fs::read(&cache).unwrap(), before);
        assert_eq!(ok(root, &["show", id])["ticket"], newer["ticket"]);
        let beat = ok(root, &["beat", id]);
        assert_eq!(
            beat["fence"], newer["fence"],
            "next command uses the new cache"
        );
        assert_eq!(ok(root, &["verify"])["entries"], 5);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_retry_freezes_absent_fence_and_distinguishes_null_progress() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        ok(root, &["init", "--project", "demo"]);
        let made = ok(root, &["new", "originally unheld"]);
        let id = made["id"].as_str().unwrap();
        let note = ["note", id, "before claim", "--request-id", "note-unheld"];
        let original = ok(root, &note);
        let missing = ["release", id, "--request-id", "missing-fence"];
        assert_eq!(run(root, Some("worker"), false, &missing).0, EXIT_CONFLICT);
        ok(root, &["claim", id]);
        assert_eq!(ok(root, &note)["seq"], original["seq"]);
        assert_eq!(ok(root, &note)["replayed"], true);
        let (code, out, _) = run(root, Some("worker"), false, &missing);
        assert_eq!(
            code, EXIT_CONFLICT,
            "the later cached fence cannot change original input: {out}"
        );
        assert_eq!(json(&out)["error"]["code"], "stale_fence");
        ok(root, &["beat", id, "--progress", "{\"step\":1}"]);
        let absent = ["beat", id, "--request-id", "keep-progress"];
        assert_eq!(ok(root, &absent)["ticket"]["progress"]["step"], 1);
        let (code, out, _) = run(
            root,
            Some("worker"),
            false,
            &[
                "beat",
                id,
                "--request-id",
                "keep-progress",
                "--progress",
                "null",
            ],
        );
        assert_eq!(code, EXIT_CONFLICT, "{out}");
        assert_eq!(json(&out)["error"]["code"], "request_conflict");
        assert!(ok(root, &["beat", id, "--progress", "null"])["ticket"]["progress"].is_null());
        let replay = ok(root, &absent);
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["ticket"]["progress"]["step"], 1);
        assert!(ok(root, &["show", id])["ticket"]["progress"].is_null());
    })
    .await;
}
