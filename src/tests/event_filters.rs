use super::*;
use crate::EventFilter;

#[tokio::test(flavor = "multi_thread")]
async fn event_filters_precede_limits_and_cursors_preserve_the_full_chain() {
    let f = or_skip!(fixture());
    let owner = Actor::human("owner");
    let reviewer = Actor::agent("reviewer");
    let id = f.new_ticket(&owner, "Audit me");
    for n in 0..205 {
        f.board
            .add_note(
                if n == 204 { &reviewer } else { &owner },
                &id,
                "note",
                "Recorded",
                None,
                None,
            )
            .unwrap();
    }
    let before = f.board.events(0, usize::MAX).unwrap();
    assert_eq!(before.len(), 206);
    let only_reviewer = f
        .board
        .events_matching(&EventFilter {
            actor: Some("reviewer".into()),
            limit: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(only_reviewer, vec![before[205].clone()]);
    let newest = f
        .board
        .events_matching(&EventFilter {
            descending: true,
            limit: Some(2),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        newest.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![206, 205]
    );
    let older = f
        .board
        .events_matching(&EventFilter {
            descending: true,
            before_seq: Some(205),
            after_seq: 201,
            ticket: Some(id.clone()),
            action_prefix: Some("note.*".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        older.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![204, 203, 202]
    );
    for prefix in ["note.%", "note_", "unrelated"] {
        assert!(
            f.board
                .events_matching(&EventFilter {
                    action_prefix: Some(prefix.into()),
                    ..Default::default()
                })
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        f.board
            .events_matching(&EventFilter {
                ticket: Some("missing".into()),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.board
            .events_matching(&EventFilter::default())
            .unwrap()
            .len(),
        200
    );
    for filter in [
        EventFilter {
            after_seq: -1,
            ..Default::default()
        },
        EventFilter {
            before_seq: Some(0),
            ..Default::default()
        },
    ] {
        assert_eq!(
            f.board.events_matching(&filter).unwrap_err().code,
            "invalid"
        );
    }
    assert_eq!(f.board.events(0, usize::MAX).unwrap(), before);
    let verified = f.board.verify_chain().unwrap();
    assert!(verified.ok);
    assert_eq!(
        verified.entries, 206,
        "verification must not silently stop at a page limit"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn event_filters_cannot_read_other_boards_or_tenants() {
    let f = or_skip!(fixture());
    let owner = Actor::human("owner");
    f.new_ticket(&owner, "Local work");
    for (tenant, project) in [
        (LOCAL_TENANT, "other"),
        ("t-00000000-0000-4000-8000-000000000001", PROJECT),
    ] {
        let foreign =
            crate::hosted::HostedBoard::new(f.test.store.clone(), tenant, project).unwrap();
        foreign.initialize().await.unwrap();
        foreign
            .create(
                &Actor::human("foreign"),
                &NewTicket {
                    title: "Private work".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let filter = EventFilter {
            actor: Some("foreign".into()),
            descending: true,
            ..Default::default()
        };
        assert_eq!(foreign.events_matching(&filter).await.unwrap().len(), 1);
        assert!(f.board.events_matching(&filter).unwrap().is_empty());
        assert_eq!(
            f.board
                .events_matching(&EventFilter::default())
                .unwrap()
                .len(),
            1
        );
    }
}
