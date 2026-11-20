use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn creation_dependencies_are_atomic_scoped_and_block_claims() {
    let fixture = or_skip!(fixture());
    let board = &fixture.board;
    let human = Actor::human("owner");
    let agent = Actor::agent("worker");
    let dependency = board
        .create(
            &human,
            NewTicket {
                title: "First".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let ticket = board
        .create(
            &human,
            NewTicket {
                title: "Second".into(),
                depends_on: vec![dependency.id.clone(), dependency.id.clone()],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(ticket.blocked_by, vec![dependency.id.clone()]);
    assert_eq!(board.get(&ticket.id).unwrap().deps.len(), 1);
    assert_eq!(
        board.claim(&agent, &ticket.id, None).unwrap_err().code,
        "blocked"
    );
    let before = board.events(0, 100).unwrap();
    let count = board.list(&Filter::default()).unwrap().len();
    let parent = board.get(&dependency.id).unwrap().ticket;
    assert!(
        board
            .create(
                &human,
                NewTicket {
                    title: "Must not exist".into(),
                    parent: Some(dependency.id.clone()),
                    depends_on: vec![dependency.id.clone(), "missing-ticket".into()],
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert_eq!(board.list(&Filter::default()).unwrap().len(), count);
    assert_eq!(board.events(0, 100).unwrap(), before);
    assert_eq!(board.get(&dependency.id).unwrap().ticket, parent);
    assert!(board.verify_chain().unwrap().ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn source_replay_includes_the_normalized_dependency_set() {
    let fixture = or_skip!(fixture());
    let board = &fixture.board;
    let human = Actor::human("owner");
    let first = board
        .create(
            &human,
            NewTicket {
                title: "First".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let second = board
        .create(
            &human,
            NewTicket {
                title: "Second".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let mut new = NewTicket {
        title: "Imported".into(),
        depends_on: vec![first.id.clone(), second.id.clone()],
        source: Some(crate::TicketSource {
            path: "plan.md".into(),
            item: "task-1".into(),
            revision: None,
            content_sha256: None,
        }),
        ..Default::default()
    };
    let made = board.create(&human, new.clone()).unwrap();
    let before = board.events(0, 100).unwrap();
    new.depends_on = vec![second.id, first.id.clone(), first.id];
    assert_eq!(board.create(&human, new.clone()).unwrap().id, made.id);
    assert_eq!(board.events(0, 100).unwrap(), before);
    new.depends_on.clear();
    assert_eq!(
        board.create(&human, new).unwrap_err().code,
        "source_conflict"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn creation_dependencies_cannot_reach_another_board_or_tenant() {
    let fixture = or_skip!(fixture());
    let board = &fixture.board;
    let human = Actor::human("owner");
    for (tenant, project) in [
        (LOCAL_TENANT, "other"),
        ("t-00000000-0000-4000-8000-000000000001", PROJECT),
    ] {
        let foreign =
            crate::hosted::HostedBoard::new(fixture.test.store.clone(), tenant, project).unwrap();
        foreign.initialize().await.unwrap();
        let ticket = foreign
            .create(
                &human,
                &NewTicket {
                    title: "Private".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let before = board.events(0, 100).unwrap();
        assert_eq!(
            board
                .create(
                    &human,
                    NewTicket {
                        title: "Must not exist".into(),
                        depends_on: vec![ticket.id],
                        ..Default::default()
                    }
                )
                .unwrap_err()
                .code,
            "not_found"
        );
        assert!(board.list(&Filter::default()).unwrap().is_empty());
        assert_eq!(board.events(0, 100).unwrap(), before);
    }
}
