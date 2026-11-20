//! A ticket's import source: once per board, an exact repeat returns the ticket as it is now
//! (no event, no change notice, its lease and edits kept), changed inputs are
//! `source_conflict` with nothing written, concurrent importers make one ticket, and the
//! recorded provenance never changes.

use std::sync::Arc;

use super::{PROJECT, fixture, or_skip, quiet};
use crate::{Actor, Filter, LOCAL_TENANT, NewTicket, Patch, TicketSource};

fn imported() -> NewTicket {
    NewTicket {
        title: "Implement project scanner".into(),
        description: "Preview discovered roadmap items before admission".into(),
        labels: vec![" Roadmap ".into(), "roadmap".into()],
        source: Some(TicketSource {
            path: "docs/ROADMAP.md".into(),
            item: "scanner-preview".into(),
            revision: Some("commit:0123456789abcdef".into()),
            content_sha256: Some("a".repeat(64)),
        }),
        ..NewTicket::default()
    }
}

/// Nothing reaches the board change channel within a short wait.

#[tokio::test(flavor = "multi_thread")]
async fn a_repeat_returns_the_ticket_as_it_is_and_changed_inputs_conflict() {
    let f = or_skip!(fixture());
    let importer = Actor::agent("importer");
    let first = f.board.create(&importer, imported()).unwrap();
    let evidence = f.board.get(&first.id).unwrap().provenance.unwrap();
    assert_eq!(evidence.recorded_by, "importer");
    assert_eq!(evidence.recorded_at, first.created_at);
    assert_eq!(Some(&evidence.source), imported().source.as_ref());
    assert_eq!(evidence.request_sha256.len(), 64);
    let created = f.board.events(0, 10).unwrap().remove(0);
    assert_eq!(created.action, "ticket.created");
    assert_eq!(
        created.data["provenance"],
        serde_json::to_value(&evidence).unwrap()
    );
    // Edited and held since: a repeat must give it back as it is, not as it was.
    f.board
        .update(
            &importer,
            &first.id,
            first.version,
            Patch {
                title: Some("Operator's refined scope".into()),
                ..Patch::default()
            },
        )
        .unwrap();
    let lease = f
        .board
        .claim(&Actor::agent("worker"), &first.id, None)
        .unwrap();
    let head = f.board.verify_chain().unwrap();

    // Another process, another importer: the board, opened again.
    let board = crate::open_board(&f.test.url, PROJECT, false)
        .await
        .unwrap();
    let mut listener = super::board_changes(&f.test.url).await;
    let retry = board.create(&Actor::agent("another"), imported()).unwrap();
    assert_eq!(retry, lease.ticket, "the ticket as it is now");
    assert_eq!(retry.holder.as_deref(), Some("worker"));
    assert_eq!(retry.fence, lease.fence);
    assert_eq!(
        board.get(&first.id).unwrap().provenance,
        Some(evidence.clone())
    );
    for change in 0..8 {
        let mut request = imported();
        fn source(r: &mut NewTicket) -> &mut TicketSource {
            r.source.as_mut().unwrap()
        }
        match change {
            0 => request.title = "Changed proposal".into(),
            1 => request.description.push('!'),
            2 => request.priority = Some(0),
            3 => request.assignee = Some("someone-else".into()),
            4 => request.labels.push("new-label".into()),
            5 => source(&mut request).revision = Some("commit:fedcba9876543210".into()),
            6 => source(&mut request).content_sha256 = Some("b".repeat(64)),
            _ => source(&mut request).revision = None,
        }
        let refused = board.create(&importer, request).unwrap_err();
        assert_eq!(refused.code, "source_conflict", "change {change}");
        assert!(refused.message.contains(&first.id), "{}", refused.message);
    }
    assert_eq!(board.list(&Filter::default()).unwrap(), vec![retry.clone()]);
    assert_eq!(board.verify_chain().unwrap(), head);
    assert!(
        quiet(&mut listener).await,
        "neither a repeat nor a conflict announces a change"
    );
    assert_eq!(board.get(&first.id).unwrap().provenance, Some(evidence));

    // The same document and item on another board is that board's own ticket.
    let other = crate::open_board(&f.test.url, "other", true).await.unwrap();
    let second = other.create(&importer, imported()).unwrap();
    assert_eq!(
        other.list(&Filter::default()).unwrap(),
        vec![second.clone()]
    );
    assert_eq!(board.list(&Filter::default()).unwrap(), vec![retry]);
    assert_eq!(
        other
            .get(&second.id)
            .unwrap()
            .provenance
            .unwrap()
            .recorded_at,
        second.created_at
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_importers_make_one_ticket_and_one_event() {
    let f = or_skip!(fixture());
    let barrier = Arc::new(tokio::sync::Barrier::new(6));
    let mut importers = Vec::new();
    for n in 0..6 {
        // Each its own pool, as six processes would be.
        let board = crate::open_board(&f.test.url, PROJECT, false)
            .await
            .unwrap();
        let barrier = barrier.clone();
        importers.push(tokio::spawn(async move {
            let name = format!("importer-{n}");
            barrier.wait().await;
            board
                .hosted()
                .create(&Actor::agent(&name), &imported())
                .await
                .unwrap()
                .id
        }));
    }
    let mut ids = Vec::new();
    for importer in importers {
        ids.push(importer.await.unwrap());
    }
    assert!(ids.iter().all(|id| id == &ids[0]), "{ids:?}");
    assert_eq!(f.board.list(&Filter::default()).unwrap().len(), 1);
    assert_eq!(f.board.events(0, 100).unwrap().len(), 1);
    assert!(f.board.verify_chain().unwrap().ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_sources_and_failed_dependencies_write_nothing_and_the_record_never_changes() {
    let f = or_skip!(fixture());
    let importer = Actor::agent("importer");
    for path in [
        "../roadmap.md",
        "/tmp/roadmap.md",
        "D:/roadmap.md",
        "docs\\roadmap.md",
        "docs//roadmap.md",
        "./ROADMAP.md",
        "docs/../ROADMAP.md",
        " docs/ROADMAP.md",
        "",
    ] {
        let mut request = imported();
        request.source.as_mut().unwrap().path = path.into();
        let refused = f.board.create(&importer, request).unwrap_err();
        assert_eq!(refused.code, "invalid", "{path:?}");
    }
    for (item, revision, digest) in [
        ("", None, None),
        ("x\n", None, None),
        ("x", Some(" r"), None),
        ("x", None, Some("g".repeat(64))),
        ("x", None, Some("A".repeat(64))),
        ("x", None, Some("a".repeat(63))),
    ] {
        let mut request = imported();
        let source = request.source.as_mut().unwrap();
        source.item = item.into();
        source.revision = revision.map(str::to_string);
        source.content_sha256 = digest;
        assert_eq!(
            f.board.create(&importer, request).unwrap_err().code,
            "invalid"
        );
    }
    let mut broken = imported();
    broken.discovered_from = Some("does-not-exist".into());
    assert_eq!(
        f.board.create(&importer, broken).unwrap_err().code,
        "not_found"
    );
    assert!(f.board.list(&Filter::default()).unwrap().is_empty());
    assert_eq!(f.board.verify_chain().unwrap().entries, 0);
    // The failed import left no source behind: the same source makes its ticket now.
    let ticket = f.board.create(&importer, imported()).unwrap();

    let store = &f.test.store;
    let tenant = |sql: &'static str| {
        f.board.hosted().blocking(async move {
            devagotchi_store::transaction!(store, |connection| {
                store.scope(connection, LOCAL_TENANT).await?;
                sqlx::query(sql).execute(&mut *connection).await?;
                Ok(())
            })
        })
    };
    assert!(tenant("UPDATE board_sources SET item = 'forged'").is_err());
    assert!(tenant("UPDATE board_sources SET recorded_by = 'operator'").is_err());
    assert!(tenant("DELETE FROM board_sources").is_err());
    assert!(
        tenant(
            "INSERT INTO board_sources SELECT tenant_id, board_id, ticket, 'elsewhere.md', item,
             revision, content_sha256, 'mallory', recorded_at, 'forged' FROM board_sources
             ON CONFLICT (tenant_id, board_id, ticket) DO UPDATE SET path = excluded.path"
        )
        .is_err()
    );
    assert!(
        tenant(
            "INSERT INTO board_sources SELECT tenant_id, board_id, ticket, path, item, revision,
             content_sha256, 'mallory', recorded_at, 'forged' FROM board_sources
             ON CONFLICT (tenant_id, board_id, path, item) DO UPDATE SET recorded_by = 'mallory'"
        )
        .is_err()
    );
    let kept = f.board.get(&ticket.id).unwrap().provenance.unwrap();
    assert_eq!(
        (kept.source.path.as_str(), kept.source.item.as_str()),
        ("docs/ROADMAP.md", "scanner-preview")
    );
    assert_eq!(kept.recorded_by, "importer");
}

#[test]
fn a_source_cannot_name_who_recorded_it() {
    let mut source = serde_json::to_value(imported().source.unwrap()).unwrap();
    source["recorded_by"] = serde_json::json!("operator");
    assert!(serde_json::from_value::<TicketSource>(source).is_err());
    // Revision and digest are optional; path and item are not.
    let minimal: TicketSource =
        serde_json::from_value(serde_json::json!({"path": "a.md", "item": "x"})).unwrap();
    assert_eq!((minimal.revision, minimal.content_sha256), (None, None));
    assert!(serde_json::from_value::<TicketSource>(serde_json::json!({"path": "a.md"})).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_normalized_repeat_survives_changed_board_defaults() {
    let f = or_skip!(fixture());
    let scanner = Actor::agent("scanner");
    let original = f.board.create(&scanner, imported()).unwrap();
    let mut settings = f.board.settings().unwrap();
    settings.default_priority = 9;
    f.board
        .put_settings(&Actor::human("operator"), &settings)
        .unwrap();
    let head = f.board.verify_chain().unwrap();
    let mut equivalent = imported();
    equivalent.title = format!("  {}  ", equivalent.title);
    equivalent.kind = Some("task".into());
    equivalent.labels = vec!["ROADMAP".into()];
    // Where it was made from is not what was asked for: other anchors are a repeat too.
    equivalent.anchors.machine = Some("another-machine".into());
    equivalent.anchors.session = Some("another-session".into());
    equivalent.anchors.agent = Some("another-seat".into());
    equivalent.anchors.repo = Some("https://github.com/x/y".into());
    assert_eq!(f.board.create(&scanner, equivalent).unwrap(), original);
    assert_eq!(f.board.verify_chain().unwrap(), head);
}

/// The digest's bytes are pinned: sorted keys, compact, whatever serde_json features a build
/// turns on. A stored `request_sha256` must keep matching its repeat.
#[test]
fn the_request_digest_hashes_a_fixed_form() {
    let mut new = imported();
    new.priority = Some(2);
    let labels = ["roadmap".to_string()];
    let digest = crate::provenance::request_digest(&new, "T", "task", &labels, None, Some("f"));
    let form = concat!(
        r#"{"assignee":null,"description":"Preview discovered roadmap items before admission","#,
        r#""discovered_from":null,"environment":null,"fingerprint":"f","kind":"task","#,
        r#""labels":["roadmap"],"parent":null,"priority":2,"source":{"content_sha256":""#,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        r#"","item":"scanner-preview","path":"docs/ROADMAP.md","#,
        r#""revision":"commit:0123456789abcdef"},"title":"T","version":1}"#,
    );
    use sha2::Digest;
    assert_eq!(digest, hex::encode(sha2::Sha256::digest(form.as_bytes())));
    new.source.as_mut().unwrap().content_sha256 = None;
    let bare = crate::provenance::request_digest(&new, "T", "task", &labels, None, Some("f"));
    let form = form.replace(&format!(r#""content_sha256":"{}","#, "a".repeat(64)), "");
    assert_eq!(bare, hex::encode(sha2::Sha256::digest(form.as_bytes())));
}
