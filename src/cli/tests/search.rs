use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn ls_text_search_is_independent_of_json_or_human_output() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(
            run(d, None, false, &["init", "--project", "demo"]).0,
            EXIT_OK
        );
        let (code, out, error) = run(d, None, false, &["new", "Needle task"]);
        assert_eq!(code, EXIT_OK, "{error}");
        let id = json(&out)["id"].as_str().unwrap().to_string();
        assert_eq!(run(d, None, false, &["new", "Unrelated task"]).0, EXIT_OK);
        assert_eq!(
            run(d, None, false, &["note", &id, "Comment evidence"]).0,
            EXIT_OK
        );
        for args in [
            vec!["ls", "--text", "Comment evidence"],
            vec!["ls", "--text=Needle"],
            vec!["ls", "--text", &id],
        ] {
            let (code, out, error) = run(d, None, false, &args);
            assert_eq!(code, EXIT_OK, "{error}");
            let result = json(&out);
            assert_eq!(result.as_array().unwrap().len(), 1, "{out}");
            assert_eq!(result[0]["id"], id);
        }
        let (code, out, error) = run(d, None, false, &["ls", "--text", "Needle", "--human"]);
        assert_eq!(code, EXIT_OK, "{error}");
        assert!(
            out.contains("Needle task") && !out.contains("Unrelated task"),
            "{out}"
        );
        assert!(!out.trim_start().starts_with('['), "human output: {out}");
        let (code, out, error) = run(d, None, false, &["ls", "--text"]);
        assert_eq!(code, EXIT_USAGE, "{out}");
        assert!(error.contains("--text needs a value"), "{error}");
        let (code, out, error) = run(d, None, false, &["show", &id, "--text"]);
        assert_eq!(code, EXIT_OK, "{error}");
        assert!(
            out.contains("Needle task") && !out.trim_start().starts_with('{'),
            "{out}"
        );
    })
    .await;
}
