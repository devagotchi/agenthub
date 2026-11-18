//! A project's standalone board (ADR 0002, ADR 0005): `<project>/.devagotchi/config.json` names
//! the project, the prefix its ids carry, the hub it is bound to, if any, and the Postgres its
//! board is in, when the project names one. The board itself is AgentHub's tables
//! (`migrations/`) in that Postgres, under one fixed local tenant, the project's name its id.

use std::path::Path;

use devagotchi_contracts::{BridgeError, BridgeResult};
use serde::{Deserialize, Serialize};

use crate::{Board, hosted::HostedBoard, migrations::MIGRATIONS, valid_name};

pub const BOARD_DIR: &str = ".devagotchi";

/// The Postgres every standalone board on this machine is in: a `postgres://` URL. A project's
/// `config.json` may name one instead (`storage`), without a password; this wins over it.
pub const STORAGE_ENV: &str = "DEVAGOTCHI_BOARD_STORAGE_URL";

/// The tenant every standalone board is under. A hub's boards are under their accounts'
/// tenants, so a standalone board in a hub's database is never one of the hub's.
pub const LOCAL_TENANT: &str = "t-00000000-0000-0000-0000-000000000000";

/// `prefix-xxxx` from the project name: lowercase letters and digits, at most four.
pub fn default_prefix(project: &str) -> String {
    let prefix: String = project
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .take(4)
        .collect();
    if prefix.is_empty() {
        "tk".into()
    } else {
        prefix
    }
}

/// A board name from a directory's name: lowercase letters, digits and single dashes, at most
/// 63, so `My_Project` is `my-project`; `project` when nothing is left.
pub fn project_name_from(dir: &str) -> String {
    let mut name = String::new();
    for c in dir.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    let name: String = name.chars().take(63).collect();
    let name = name.trim_end_matches('-');
    if name.is_empty() {
        "project".into()
    } else {
        name.into()
    }
}

/// What `board init` writes in `.devagotchi/`, so every face of the board (command line
/// here, the hub's tools) opens it under the name its ids were minted with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project: String,
    pub prefix: String,
    pub created_at: i64,
    /// The hub this project is bound to (B11): with an account on file, every board
    /// command runs there as the account; absent, the board is the standalone one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// The Postgres the standalone board is in, when this project names one (`board init
    /// --storage`): a `postgres://` URL with no password in it (the password comes from
    /// `PGPASSWORD` or the user's `.pgpass`). `DEVAGOTCHI_BOARD_STORAGE_URL` wins over it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
}

pub fn read_project_config(board_dir: &Path) -> BridgeResult<ProjectConfig> {
    let path = board_dir.join("config.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| BridgeError::new("no_board", format!("{}: {e}", path.display())))?;
    serde_json::from_str(&raw)
        .map_err(|e| BridgeError::new("board_io", format!("config.json: {e}")))
}

/// A `postgres://` URL a project's `config.json` may hold: no password, which would be read by
/// anyone who reads the project.
pub fn check_storage(url: &str) -> BridgeResult<()> {
    let refuse = |why: &str| BridgeError::new("invalid_storage", format!("storage {why}"));
    let parsed = reqwest::Url::parse(url).map_err(|_| refuse("is not a URL"))?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") {
        return Err(refuse("must be a postgres:// URL"));
    }
    if parsed.password().is_some() {
        return Err(refuse(
            "has a password in it: keep it out of the project (PGPASSWORD, ~/.pgpass, or DEVAGOTCHI_BOARD_STORAGE_URL)",
        ));
    }
    Ok(())
}

/// `url` as messages show it: without its password.
pub fn shown_storage(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        Err(_) => "(an unreadable URL)".into(),
    }
}

/// Why a standalone board cannot be opened: nowhere to open it.
pub fn no_storage() -> BridgeError {
    BridgeError::new(
        "no_storage",
        format!(
            "a standalone board is in Postgres and none is named: set {STORAGE_ENV} to a postgres:// URL, or `devagotchi board bind --hub URL` to work on a hub"
        ),
    )
}

/// The Postgres a project's standalone board is in: `env` (`DEVAGOTCHI_BOARD_STORAGE_URL`), or
/// the project's own `storage`, which must carry no password.
pub fn storage_url(env: Option<&str>, configured: Option<&str>) -> BridgeResult<String> {
    if let Some(url) = env.map(str::trim).filter(|url| !url.is_empty()) {
        return Ok(url.to_string());
    }
    if let Some(url) = configured.map(str::trim).filter(|url| !url.is_empty()) {
        check_storage(url)?;
        return Ok(url.to_string());
    }
    Err(no_storage())
}

/// The board of `project` in the Postgres at `url`, under the local tenant, its tables brought
/// up to date with AgentHub's migrations. With `create` it is made if it is not there yet
/// (`board init`); without, a board nobody made is `no_board`.
pub async fn open_board(url: &str, project: &str, create: bool) -> BridgeResult<Board> {
    if !valid_name(project) {
        return Err(BridgeError::new(
            "invalid",
            format!(
                "project name {project:?} is not 1 to 63 of [a-z0-9-] starting with a letter or digit; pass --project"
            ),
        ));
    }
    let store = devagotchi_store::Store::open(url, &[&MIGRATIONS])
        .await
        .map_err(|error| {
            BridgeError::new(
                "board_store",
                format!("board storage {}: {}", shown_storage(url), error.message),
            )
        })?;
    let hosted = HostedBoard::new(store, LOCAL_TENANT, project)?;
    if create {
        hosted.initialize().await?;
    }
    Board::from_hosted(hosted).await.map_err(|error| {
        if error.code == "not_found" {
            BridgeError::new(
                "no_board",
                format!(
                    "no board {project:?} in {}; run `devagotchi board init` in the project",
                    shown_storage(url)
                ),
            )
        } else {
            error
        }
    })
}

/// The runtime a synchronous caller (the command line) opens a board on and keeps until the
/// board is dropped: the board's Postgres pool runs on it.
pub fn runtime() -> BridgeResult<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| BridgeError::new("board_runtime", error.to_string()))
}

/// Open an initialised project's board: the project named in `<project_dir>/.devagotchi/
/// config.json`, in the Postgres `storage_env` (`DEVAGOTCHI_BOARD_STORAGE_URL`) or that file
/// names, on `runtime`. An uninitialised directory is `no_board`, never silently created:
/// `board init` is the deliberate act that also writes the ignore file and the AGENTS.md block.
pub fn open_project(
    project_dir: &Path,
    storage_env: Option<&str>,
    runtime: &tokio::runtime::Runtime,
) -> BridgeResult<(ProjectConfig, Board)> {
    let board_dir = project_dir.join(BOARD_DIR);
    if !board_dir.join("config.json").is_file() {
        return Err(BridgeError::new(
            "no_board",
            format!(
                "no board under {}; run `devagotchi board init --dir {}` there",
                project_dir.display(),
                project_dir.display()
            ),
        ));
    }
    let config = read_project_config(&board_dir)?;
    let url = storage_url(storage_env, config.storage.as_deref())?;
    let board = runtime.block_on(open_board(&url, &config.project, false))?;
    Ok((config, board))
}
