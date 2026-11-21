use super::*;
use crate::cli::{CachedClaim, args::parse};

fn parsed(args: &[&str]) -> Parsed {
    parse(
        &args
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn selection() -> Selection {
    Selection {
        key: "same-request".into(),
        operation: "release",
    }
}

#[test]
fn preparation_freezes_fence_session_and_preserves_unrelated_ignore_rules() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".gitignore"), "other-private-file").unwrap();
    let raw = parsed(&["release", "demo-1", "Done", "--request-id", "same-request"]);
    let original = intent(&raw);
    let mut cache = ClaimCache::default();
    cache.claims.insert(
        "demo-1".into(),
        CachedClaim {
            fence: 7,
            version: 2,
            lease_expires_at: 100,
        },
    );
    let scope = json!({"principal":"owner","project":"demo"});
    {
        let prepared = prepare(dir.path(), &scope, &selection(), &original, &raw, &cache).unwrap();
        assert_eq!(prepared.parsed.flag("--fence"), Some("7"));
    }
    cache.claims.clear();
    {
        let prepared = prepare(dir.path(), &scope, &selection(), &original, &raw, &cache).unwrap();
        assert_eq!(prepared.parsed.flag("--fence"), Some("7"));
    }
    cache.claims.insert(
        "demo-1".into(),
        CachedClaim {
            fence: 9,
            version: 3,
            lease_expires_at: 200,
        },
    );
    {
        let prepared = prepare(dir.path(), &scope, &selection(), &original, &raw, &cache).unwrap();
        assert_eq!(prepared.parsed.flag("--fence"), Some("7"));
    }
    let mut raw = parsed(&["new", "New task", "--request-id", "new-request"]);
    let original = intent(&raw);
    raw.flags
        .insert("--session".into(), vec!["first-session".into()]);
    let selection = select(&raw).unwrap().unwrap();
    drop(prepare(dir.path(), &scope, &selection, &original, &raw, &cache).unwrap());
    raw.flags
        .insert("--session".into(), vec!["second-session".into()]);
    let prepared = prepare(dir.path(), &scope, &selection, &original, &raw, &cache).unwrap();
    assert_eq!(prepared.parsed.flag("--session"), Some("first-session"));
    assert_eq!(
        fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
        "other-private-file\nrequests/\n"
    );
}

#[test]
fn preparation_refuses_changed_input_damage_and_unwritable_state() {
    let dir = tempfile::tempdir().unwrap();
    let raw = parsed(&["new", "Original"]);
    let scope = json!({"principal":"one"});
    drop(
        prepare(
            dir.path(),
            &scope,
            &selection(),
            &intent(&raw),
            &raw,
            &ClaimCache::default(),
        )
        .unwrap(),
    );
    let different = parsed(&["new", "Changed"]);
    let error = prepare(
        dir.path(),
        &scope,
        &selection(),
        &intent(&different),
        &different,
        &ClaimCache::default(),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "request_conflict");
    // Another authenticated principal has an independent preparation record.
    drop(
        prepare(
            dir.path(),
            &json!({"principal":"two"}),
            &selection(),
            &intent(&different),
            &different,
            &ClaimCache::default(),
        )
        .unwrap(),
    );
    for entry in fs::read_dir(dir.path().join("requests")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            fs::write(path, b"{").unwrap();
        }
    }
    assert_eq!(
        prepare(
            dir.path(),
            &scope,
            &selection(),
            &intent(&raw),
            &raw,
            &ClaimCache::default()
        )
        .err()
        .unwrap()
        .code,
        "board_io"
    );
    let blocked = tempfile::tempdir().unwrap();
    fs::write(blocked.path().join("requests"), b"existing file").unwrap();
    assert_eq!(
        prepare(
            blocked.path(),
            &scope,
            &selection(),
            &intent(&raw),
            &raw,
            &ClaimCache::default()
        )
        .err()
        .unwrap()
        .code,
        "board_io"
    );
}

#[test]
fn concurrent_preparation_selects_one_original_default_and_excludes_storage_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let mut threads = Vec::new();
    for fence in [7, 9] {
        let root = dir.path().to_path_buf();
        threads.push(std::thread::spawn(move || {
            let raw = parsed(&[
                "release",
                "demo-1",
                "--storage",
                "postgres://secret:password@host/db",
            ]);
            let mut cache = ClaimCache::default();
            cache.claims.insert(
                "demo-1".into(),
                CachedClaim {
                    fence,
                    version: 2,
                    lease_expires_at: 100,
                },
            );
            let prepared = prepare(
                &root,
                &json!({"actor":"same"}),
                &selection(),
                &intent(&raw),
                &raw,
                &cache,
            )
            .unwrap();
            assert!(prepared.parsed.flag("--storage").is_none());
            prepared.parsed.flag("--fence").unwrap().to_owned()
        }));
    }
    let values: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(values[0], values[1]);
    for entry in fs::read_dir(dir.path().join("requests")).unwrap() {
        assert!(
            !fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .contains("password")
        );
    }
}

#[test]
fn reads_batches_and_malformed_keys_are_not_mutation_requests() {
    for args in [
        vec!["show", "demo-1"],
        vec!["beat"],
        vec!["scan", "--import"],
        vec!["settings"],
        vec!["hook", "beat"],
    ] {
        let mut args = args;
        args.extend(["--request-id", "one"]);
        assert_eq!(select(&parsed(&args)).err().unwrap().code, "usage");
    }
    assert!(
        select(&parsed(&["new", "Title"]))
            .unwrap()
            .unwrap()
            .key
            .starts_with("cli-")
    );
    assert_eq!(
        select(&parsed(&["new", "Title", "--request-id", ""]))
            .err()
            .unwrap()
            .code,
        "usage"
    );
}
