//! A project's standalone board (`project.rs`): where its Postgres is named, what a project
//! may keep in its own files, the name a directory gives it, and opening it from the project.

use crate::{
    ProjectConfig, check_storage, no_storage, open_project, project_name_from, shown_storage,
    storage_url,
};

#[test]
fn the_storage_is_the_environments_else_the_projects_without_a_password() {
    let env = "postgres://board:secret@db.example:5432/boards";
    let mine = "postgres://board@db.example:5432/mine";
    assert_eq!(
        storage_url(Some(env), Some(mine)).unwrap(),
        env,
        "the environment wins"
    );
    assert_eq!(storage_url(Some("  "), Some(mine)).unwrap(), mine);
    assert_eq!(storage_url(None, Some(mine)).unwrap(), mine);
    assert_eq!(
        storage_url(None, Some(env)).unwrap_err().code,
        "invalid_storage",
        "a project's file never holds a password"
    );
    let none = storage_url(None, None).unwrap_err();
    assert_eq!(none.code, "no_storage");
    assert!(
        none.message.contains("DEVAGOTCHI_BOARD_STORAGE_URL")
            && none.message.contains("board bind"),
        "{}",
        none.message
    );
    assert_eq!(no_storage().code, "no_storage");
    assert!(check_storage(mine).is_ok());
    assert!(check_storage("postgresql://board@db.example/mine").is_ok());
    for refused in ["mysql://board@db.example/mine", "not a url", env] {
        assert_eq!(
            check_storage(refused).unwrap_err().code,
            "invalid_storage",
            "{refused}"
        );
    }
    let shown = shown_storage(env);
    assert!(!shown.contains("secret"), "{shown}");
    assert!(shown.contains("db.example:5432/boards"), "{shown}");
}

#[test]
fn a_directory_names_a_board_in_lowercase_letters_digits_and_dashes() {
    for (dir, name) in [
        ("devagotchi", "devagotchi"),
        ("My_Project", "my-project"),
        (".tmpAb3__x", "tmpab3-x"),
        ("--", "project"),
        ("", "project"),
    ] {
        assert_eq!(project_name_from(dir), name, "{dir:?}");
        assert!(crate::valid_name(&project_name_from(dir)));
    }
    assert_eq!(project_name_from(&"a".repeat(80)).len(), 63);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_opens_its_board_in_the_storage_it_names_and_never_makes_one() {
    let test = super::or_skip!(super::postgres());
    let url = test.url.clone();
    tokio::task::spawn_blocking(move || {
        let runtime = crate::runtime().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let refused =
            |storage_env: Option<&str>| match open_project(dir.path(), storage_env, &runtime) {
                Ok(_) => panic!("must not open"),
                Err(error) => error.code,
            };
        assert_eq!(refused(Some(&url)), "no_board", "no .devagotchi/ here");
        let board_dir = dir.path().join(crate::BOARD_DIR);
        std::fs::create_dir_all(&board_dir).unwrap();
        let write = |storage: Option<String>| {
            let config = ProjectConfig {
                project: "demo".into(),
                prefix: "demo".into(),
                created_at: 1,
                hub: None,
                storage,
            };
            std::fs::write(
                board_dir.join("config.json"),
                serde_json::to_string(&config).unwrap(),
            )
            .unwrap();
        };
        write(None);
        assert_eq!(refused(None), "no_storage");
        assert_eq!(refused(Some(&url)), "no_board", "nobody made it yet");
        runtime
            .block_on(crate::open_board(&url, "demo", true))
            .unwrap();
        let (config, board) = open_project(dir.path(), Some(&url), &runtime).unwrap();
        assert_eq!(config.project, "demo");
        assert_eq!(board.prefix(), "demo");
        drop(board);
        // Named by the project instead, with no password: the same board.
        write(Some(url.clone()));
        let (_, board) = open_project(dir.path(), None, &runtime).unwrap();
        assert_eq!(board.prefix(), "demo");
        drop(board);
        let with_password = url.replacen("postgres@", "postgres:secret@", 1);
        assert_ne!(with_password, url);
        write(Some(with_password));
        assert_eq!(refused(None), "invalid_storage");
    })
    .await
    .unwrap();
}
