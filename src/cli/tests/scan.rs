//! `board scan` on a local board: a preview, an import, and a second scan that imports nothing.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_scan_previews_imports_once_and_knows_what_is_already_on_the_board() {
    on_postgres(|| {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(
            run(root, None, false, &["init", "--project", "demo"]).0,
            EXIT_OK
        );
        std::fs::create_dir_all(root.join(".kiro/specs/push")).unwrap();
        std::fs::write(
            root.join(".kiro/specs/push/tasks.md"),
            "# Push\n- [ ] Subscribe the phone\n- [x] Pick VAPID\n",
        )
        .unwrap();
        std::fs::write(root.join("main.py"), "# TODO: retry the send\n").unwrap();

        let (code, out, err) = run(root, None, false, &["scan"]);
        assert_eq!(code, EXIT_OK, "{err}");
        let preview = json(&out);
        assert_eq!(preview["found"], 2, "{preview}");
        assert_eq!(preview["new"], 2);
        assert_eq!(preview["imported"], 0);
        assert!(preview["items"][0]["ticket"].is_null());
        let (_, listed, _) = run(root, None, false, &["ls"]);
        assert_eq!(
            json(&listed).as_array().map(Vec::len),
            Some(0),
            "a preview adds nothing"
        );

        let (code, out, err) = run(root, None, false, &["scan", "--import"]);
        assert_eq!(code, EXIT_OK, "{err}");
        let imported = json(&out);
        assert_eq!(imported["imported"], 2, "{imported}");
        let id = imported["items"][0]["ticket"].as_str().unwrap().to_string();
        let (_, shown, _) = run(root, None, false, &["show", &id]);
        let shown = json(&shown);
        let ticket = if shown["ticket"].is_object() {
            &shown["ticket"]
        } else {
            &shown
        };
        assert_eq!(ticket["title"], "Subscribe the phone", "{shown}");
        assert_eq!(ticket["labels"], serde_json::json!(["kiro", "scanned"]));
        assert!(
            ticket["description"]
                .as_str()
                .unwrap()
                .contains(".kiro/specs/push/tasks.md line 2"),
            "{shown}"
        );

        // Lines move and a new item appears: only the new one is new.
        std::fs::write(
            root.join(".kiro/specs/push/tasks.md"),
            "# Push\n\n- [ ] Approve on the phone\n- [ ] Subscribe the phone\n",
        )
        .unwrap();
        let (code, out, _) = run(root, None, true, &["scan", "--import"]);
        assert_eq!(code, EXIT_OK);
        assert!(
            out.contains(&format!(
                "= {id}  .kiro/specs/push/tasks.md:4  Subscribe the phone"
            )),
            "{out}"
        );
        assert!(
            out.trim_end()
                .ends_with("3 found in 2 files: 1 imported, 2 already on the board."),
            "{out}"
        );

        let (code, out, _) = run(root, None, false, &["scan", ".."]);
        assert_eq!(code, EXIT_USAGE, "{out}");
    })
    .await;
}
