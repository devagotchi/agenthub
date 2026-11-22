use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn held_update_cli_uses_claim_cache_and_rejects_explicit_stale_fence() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ok = |args: &[&str]| {
            let (code, out, err) = run(root, Some("worker"), false, args);
            assert_eq!(code, EXIT_OK, "{out} {err}");
            json(&out)
        };
        ok(&["init", "--project", "demo"]);
        let made = ok(&["new", "keep the fence"]);
        let id = made["id"].as_str().unwrap();
        let lease = ok(&["claim", id]);
        let version = lease["ticket"]["version"].to_string();
        let changed = ok(&[
            "set",
            id,
            "--version",
            &version,
            "--title",
            "edited under lease",
            "--progress",
            "{\"step\":2}",
        ]);
        assert_eq!(changed["title"], "edited under lease");
        assert_eq!(changed["progress"]["step"], 2);
        assert_eq!(
            changed["lease_expires_at"],
            lease["ticket"]["lease_expires_at"]
        );
        let version = changed["version"].to_string();
        let stale = (lease["fence"].as_i64().unwrap() - 1).to_string();
        let (code, out, _) = run(
            root,
            Some("worker"),
            false,
            &[
                "set",
                id,
                "--version",
                &version,
                "--fence",
                &stale,
                "--title",
                "bad",
            ],
        );
        assert_eq!(code, EXIT_CONFLICT, "{out}");
        assert_eq!(json(&out)["error"]["code"], "stale_fence");
        assert_eq!(ok(&["show", id])["ticket"]["title"], changed["title"]);
        assert_eq!(ok(&["verify"])["ok"], true);
    })
    .await;
}
