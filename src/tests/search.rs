//! Search parity with the October store: IDs, notes, web-search and literal substrings.

use super::*;

fn matches(board: &Board, query: &str) -> Vec<String> {
    board
        .list(&Filter {
            text: Some(query.into()),
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .map(|ticket| ticket.id)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn search_finds_ids_notes_substrings_and_web_search_without_mutation() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let ticket = f
        .board
        .create(
            &human,
            NewTicket {
                title: "Alpha implementation".into(),
                description: "Beta acceptance and exact phrase".into(),
                labels: vec!["release".into()],
                ..Default::default()
            },
        )
        .unwrap();
    f.new_ticket(&human, "Unrelated work");
    f.board
        .add_note(
            &human,
            &ticket.id,
            "note",
            "Migration evidence recorded",
            None,
            None,
        )
        .unwrap();
    let before = f.board.events(0, 100).unwrap();
    for query in [
        &ticket.id,
        "  migration EVIDENCE  ",
        "plement",
        "alpha beta",
        "\"exact phrase\"",
        "alpha OR nonexistent",
    ] {
        assert_eq!(matches(&f.board, query), vec![ticket.id.clone()], "{query}");
    }
    assert!(matches(&f.board, "\"alpha beta\"").is_empty());
    assert_eq!(matches(&f.board, " \t ").len(), 2);
    for (label, expected) in [("release", 1), ("other", 0)] {
        assert_eq!(
            f.board
                .list(&Filter {
                    text: Some("migration evidence".into()),
                    label: Some(label.into()),
                    ..Default::default()
                })
                .unwrap()
                .len(),
            expected
        );
    }
    assert_eq!(f.board.events(0, 100).unwrap(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn search_keeps_percent_underscore_and_backslash_literal() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let literal = f.new_ticket(&human, "Literal characters");
    let ordinary = f.new_ticket(&human, "Ordinary characters");
    f.board
        .add_note(
            &human,
            &literal,
            "note",
            r"100%_complete C:\work\report",
            None,
            None,
        )
        .unwrap();
    f.board
        .add_note(
            &human,
            &ordinary,
            "note",
            "100 percent complete C work report",
            None,
            None,
        )
        .unwrap();
    for query in ["100%", "_complete", r"C:\work", "%", "_", "\\"] {
        assert_eq!(matches(&f.board, query), vec![literal.clone()], "{query}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn search_never_matches_notes_from_another_board_or_tenant_even_with_same_id() {
    let f = or_skip!(fixture());
    let human = Actor::human("owner");
    let id = f.new_ticket(&human, "Shared ID but separate records");
    for (tenant, project) in [
        (LOCAL_TENANT, "other"),
        ("t-00000000-0000-4000-8000-000000000001", PROJECT),
    ] {
        let foreign =
            crate::hosted::HostedBoard::new(f.test.store.clone(), tenant, project).unwrap();
        foreign.initialize().await.unwrap();
        // Deliberately collide IDs: random IDs would hide a missing correlation predicate.
        sqlx::query("INSERT INTO devagotchi.board_tickets
            (tenant_id,board_id,id,kind,title,description,state,priority,labels,created_by,created_at,updated_at)
            SELECT $1,$2,id,kind,title,description,state,priority,labels,created_by,created_at,updated_at
            FROM devagotchi.board_tickets WHERE tenant_id=$3 AND board_id=$4 AND id=$5")
            .bind(tenant).bind(project).bind(LOCAL_TENANT).bind(PROJECT).bind(&id)
            .execute(f.test.store.admin()).await.unwrap();
        foreign
            .add_note(&human, &id, "note", "Confidential discovery", None, None)
            .await
            .unwrap();
        assert_eq!(
            foreign
                .list(&Filter {
                    text: Some("confidential discovery".into()),
                    ..Default::default()
                })
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(matches(&f.board, "confidential discovery").is_empty());
        assert_eq!(matches(&f.board, &id), vec![id.clone()]);
    }
}
