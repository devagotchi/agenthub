//! The scanner on a project laid out the way AI tools leave one.

use std::path::Path;

use super::{check, library, rules_for, scan};

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn every_library_rule_file_parses_and_names_what_it_reads() {
    let rules = library();
    assert!(rules.len() >= 8);
    for rule in &rules {
        assert!(!rule.name.is_empty() && !rule.label.is_empty(), "{rule:?}");
        assert_eq!(check(rule), Ok(()), "{}", rule.name);
    }
}

#[test]
fn the_library_finds_what_kiro_spec_kit_cursor_claude_plans_and_code_leave() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        ".kiro/specs/login/tasks.md",
        "# Tasks\n\n- [ ] 1. Add the sign-in form\n- [x] 2. Done already\n  - [ ] 2.1 Wire the passkey\n",
    );
    write(
        root,
        "specs/001-push/tasks.md",
        "## Phase 1\n- [ ] T001 Subscribe the phone\n",
    );
    write(root, ".cursor/plans/cache.md", "* [ ] Add Dragonfly\n");
    write(root, ".claude/plans/x.md", "1. [ ] Rename machines\n");
    write(root, "TODO.md", "- [ ] Write the README\n");
    write(
        root,
        "docs/notes.md",
        "## Ideas\n```\n- [ ] not this, it is code\n```\n- [ ] Dormant machines\n",
    );
    write(
        root,
        "src/main.rs",
        "fn main() {\n    // TODO: handle the error\n    let s = \"TODO: not a comment\";\n    /* FIXME(ann): close it */\n}\n",
    );
    write(root, "tool.py", "x = 1  # HACK: pin until 2.0\n# TODO\n");
    // Never read: dependencies, build output, hidden folders no rule names.
    write(root, "node_modules/pkg/TODO.md", "- [ ] theirs\n");
    write(root, "target/x.rs", "// TODO: built\n");
    write(root, ".idea/notes.md", "- [ ] editor\n");

    let found = scan(root, root, &library());
    let got: Vec<_> = found
        .findings
        .iter()
        .map(|f| (f.path.as_str(), f.line, f.label.as_str(), f.title.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (".claude/plans/x.md", 1, "claude", "Rename machines"),
            (".cursor/plans/cache.md", 1, "cursor", "Add Dragonfly"),
            (
                ".kiro/specs/login/tasks.md",
                3,
                "kiro",
                "1. Add the sign-in form"
            ),
            (
                ".kiro/specs/login/tasks.md",
                5,
                "kiro",
                "2.1 Wire the passkey"
            ),
            ("TODO.md", 1, "plan", "Write the README"),
            ("docs/notes.md", 5, "checklist", "Dormant machines"),
            (
                "specs/001-push/tasks.md",
                2,
                "spec-kit",
                "T001 Subscribe the phone"
            ),
            ("src/main.rs", 2, "todo", "handle the error"),
            ("src/main.rs", 4, "todo", "close it"),
            ("tool.py", 1, "todo", "pin until 2.0"),
        ]
    );
    assert_eq!(found.findings[6].heading.as_deref(), Some("Phase 1"));
    assert!(!found.truncated);
}

#[test]
fn a_fingerprint_survives_moved_lines_and_tells_repeats_apart() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "TODO.md", "- [ ] Ship it\n- [ ] ship   IT\n");
    let first = scan(root, root, &library()).findings;
    write(
        root,
        "TODO.md",
        "# Now\n\nintro\n\n- [ ] Ship it\n- [ ] ship   IT\n",
    );
    let second = scan(root, root, &library()).findings;
    assert_eq!(first.len(), 2);
    assert_ne!(first[0].fingerprint, first[1].fingerprint);
    assert_eq!(
        first.iter().map(|f| &f.fingerprint).collect::<Vec<_>>(),
        second.iter().map(|f| &f.fingerprint).collect::<Vec<_>>()
    );
    assert!(first[0].fingerprint.starts_with("scan:") && first[0].fingerprint.len() == 37);
    // A subfolder scan names paths from the project, so it finds the same fingerprints.
    write(root, "a/TODO.md", "- [ ] Inner\n");
    let whole = scan(root, root, &library());
    let part = scan(root, &root.join("a"), &library());
    assert_eq!(part.findings.len(), 1);
    assert!(whole.findings.contains(&part.findings[0]));
    let one = scan(root, &root.join("a/TODO.md"), &library());
    assert_eq!(one.findings, part.findings);
}

#[test]
fn a_project_rule_comes_first_and_a_bad_rule_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let board = root.join(".devagotchi");
    std::fs::create_dir_all(&board).unwrap();
    assert_eq!(rules_for(&board).unwrap(), library());

    std::fs::write(
        board.join("scan.json"),
        r#"[{"name":"notes","label":"note","under":["notes/"],"ext":["txt"],"kind":"marker","markers":["NOTE"]}]"#,
    )
    .unwrap();
    let rules = rules_for(&board).unwrap();
    assert_eq!(rules[0].name, "notes");
    write(root, "notes/a.txt", "# NOTE: call the bank\n");
    let found = scan(root, root, &rules);
    assert_eq!(found.findings.len(), 1);
    assert_eq!(found.findings[0].title, "call the bank");

    std::fs::write(board.join("scan.json"), r#"[{"name":"x"}]"#).unwrap();
    assert_eq!(rules_for(&board).unwrap_err().code, "invalid_scan_rules");
    std::fs::write(
        board.join("scan.json"),
        r#"[{"name":"x","label":"x","ext":["txt"],"kind":"marker"}]"#,
    )
    .unwrap();
    assert_eq!(rules_for(&board).unwrap_err().code, "invalid_scan_rules");
}

#[test]
fn a_plan_table_gives_its_open_rows_with_their_id_heading_and_status() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        ".ai/dev/plans/path.md",
        "## Phase 0: prove it

| # | Feature (operator's words) | In the code today | Proof |
|---|---|---|---|
| 0.1 | turnkey **server** | built: `install.sh` | Texas |
| 0.3 | connect a machine | partial: Linux needs root | joins |
| 0.4 | push | Partially there | buzz |

| Milestone | Task | Dependencies | Acceptance evidence | State |
| --- | --- | --- | --- | --- |
| F5 | Defaults library and importers | F1 | parser fixtures | not started |
| F12 | Agents as assignees | none | a test | done (increment 139) |
| F9 | Outcomes | F4 | | `pending` |

| Decision | Recommendation |
|---|---|
| SQLite | keep the journals |
",
    );
    let found = scan(root, root, &library()).findings;
    let got: Vec<_> = found
        .iter()
        .map(|f| {
            (
                f.line,
                f.title.as_str(),
                f.status.as_deref(),
                f.heading.as_deref(),
            )
        })
        .collect();
    let phase = Some("Phase 0: prove it");
    assert_eq!(
        got,
        vec![
            (
                6,
                "0.3 connect a machine",
                Some("partial: Linux needs root"),
                phase
            ),
            (
                11,
                "F5 Defaults library and importers",
                Some("not started"),
                phase
            ),
            (13, "F9 Outcomes", Some("`pending`"), phase),
        ]
    );
    assert!(found.iter().all(|f| f.label == "ai-plans"));
}
