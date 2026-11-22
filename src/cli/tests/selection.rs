use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_claim_labels_select_the_intersection_and_replay_the_same_lease() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let (code, out, err) = run(d, None, false, &["init", "--project", "demo"]);
        assert_eq!(code, EXIT_OK, "{out} {err}");
        let (code, out, err) = run(d, None, false, &["new", "Partial", "--label", "api"]);
        assert_eq!(code, EXIT_OK, "{out} {err}");
        let (code, out, err) = run(
            d,
            None,
            false,
            &["new", "Both", "--label", "release", "--label", "api"],
        );
        assert_eq!(code, EXIT_OK, "{out} {err}");
        let id = json(&out)["id"].as_str().unwrap().to_owned();
        let args = [
            "claim",
            "--label",
            "release",
            "--label",
            "api",
            "--request-id",
            "labels-once",
        ];
        let (code, out, err) = run(d, Some("worker"), false, &args);
        assert_eq!(code, EXIT_OK, "{out} {err}");
        let first = json(&out);
        assert_eq!(first["ticket"]["id"], id);
        let (code, out, err) = run(d, Some("worker"), false, &args);
        assert_eq!(code, EXIT_OK, "{out} {err}");
        let replay = json(&out);
        assert_eq!(first["fence"], replay["fence"]);
        assert_eq!(first["lease_expires_at"], replay["lease_expires_at"]);
    })
    .await;
}
