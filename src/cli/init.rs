//! `board init`: the project's board folder, its .gitignore, and the AGENTS.md block that teaches an agent the workflow.

use devagotchi_contracts::BridgeError;

use crate::{BOARD_DIR, BOARD_FILE, Board, ProjectConfig, default_prefix, read_project_config};

use super::{EXIT_OK, Invocation, Output, Parsed, write_json};

/// What `.devagotchi/` keeps out of the repo: the store, the claim cache, the beat markers.
pub(super) const LOCAL_GITIGNORE: &str = "# the board itself is local state; board.jsonl is the export to commit if you want diffs\nboard.sqlite3\nboard.sqlite3-wal\nboard.sqlite3-shm\nclaims/\nhooks/.last-beat-*\n";

pub(super) fn init(parsed: &Parsed, invocation: &Invocation, output: &mut Output<'_>) -> u8 {
    let project_dir = parsed
        .flag("--dir")
        .map(|d| invocation.cwd.join(d))
        .unwrap_or_else(|| invocation.cwd.clone());
    let board_dir = project_dir.join(BOARD_DIR);
    let existing = read_project_config(&board_dir).ok();
    let project = parsed
        .flag("--project")
        .map(str::to_string)
        .or_else(|| existing.as_ref().map(|c| c.project.clone()))
        .or_else(|| {
            project_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "project".into());
    let prefix = parsed
        .flag("--prefix")
        .map(str::to_string)
        .or_else(|| existing.as_ref().map(|c| c.prefix.clone()))
        .unwrap_or_else(|| default_prefix(&project));
    if let Err(error) = std::fs::create_dir_all(&board_dir) {
        return output.error(&BridgeError::new("board_io", error.to_string()), None);
    }
    let board = match Board::open(&board_dir.join(BOARD_FILE), &project) {
        Ok(b) => b,
        Err(error) => return output.error(&error, None),
    };
    let config = ProjectConfig {
        project: project.clone(),
        prefix: board.prefix().to_string(),
        created_at: existing
            .as_ref()
            .map(|c| c.created_at)
            .unwrap_or_else(|| board.now()),
        hub: existing.and_then(|c| c.hub),
    };
    if prefix != config.prefix {
        output.lines(&format!(
            "note: this board already uses prefix {}; --prefix {} ignored",
            config.prefix, prefix
        ));
    }
    if let Err(error) = write_json(&board_dir.join("config.json"), &config) {
        return output.error(&error, None);
    }
    let gitignore = board_dir.join(".gitignore");
    if !gitignore.exists() {
        let _ = std::fs::write(&gitignore, LOCAL_GITIGNORE);
    }
    let mut agents_md = "unchanged";
    if parsed.switch("--agents-md") {
        let path = project_dir.join("AGENTS.md");
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current.contains(AGENTS_MD_START) {
            agents_md = "already present";
        } else {
            let mut next = current;
            if !next.is_empty() && !next.ends_with('\n') {
                next.push('\n');
            }
            if !next.is_empty() {
                next.push('\n');
            }
            next.push_str(&agents_md_block());
            if let Err(error) = std::fs::write(&path, next) {
                return output.error(&BridgeError::new("board_io", error.to_string()), None);
            }
            agents_md = "block appended";
        }
    }
    output.value(
        &serde_json::json!({
            "project": config.project,
            "prefix": config.prefix,
            "board": board_dir.join(BOARD_FILE),
            "agents_md": agents_md,
        }),
        || {
            format!(
                "board ready: {} (ids {}-…)\n  {}\n  AGENTS.md: {}\n  next: devagotchi board prime — or board onboard to paste the block elsewhere",
                config.project,
                config.prefix,
                board_dir.join(BOARD_FILE).display(),
                agents_md
            )
        },
    );
    EXIT_OK
}

pub use crate::PROTOCOL_LINE;

pub const AGENTS_MD_START: &str = "<!-- devagotchi-board:start -->";

pub const AGENTS_MD_END: &str = "<!-- devagotchi-board:end -->";

pub fn agents_md_block() -> String {
    format!(
        "{AGENTS_MD_START}
## Board

This project's tickets live in `.devagotchi/board.sqlite3` (the agents' board; contract in the
devagotchi repository, `docs/agent-board.md`). Work from the board, not from memory:

- Start every session with `devagotchi board prime` (set `DEVAGOTCHI_ACTOR=<your-name>` first,
  for example `claude`, `codex`, `gemini`, `cursor`, `grok`): it lists what you hold, what is ready
  for you, and notes addressed to you.
- Take work with `board claim` (or `board claim <id>`), heartbeat with `board beat <id>` at least as
  often as `prime` says, record what you learn with `board note <id> \"…\"`, finish with
  `board done <id> \"summary\"`, or `board handoff <id> \"note\" --to <who>` when someone else must
  continue. Work you discover goes on the board: `board new \"title\" --found-in <id>`.
- Never stop while you hold a ticket: hand it off or finish it. Reviewers are never the author.
- Every command takes `--json`; errors carry a `next` field that says what to run.
{AGENTS_MD_END}"
    )
}

pub const AGENT_WORKFLOW: &str = "The board workflow for an agent

1. prime      devagotchi board prime                what you hold, what is ready for you, notes to you
2. claim      board claim [<id>]                   a lease with a fence; re-claiming what you hold is free
3. work       board beat <id> [--progress '{...}']  before the heartbeat interval runs out, every time
              board note <id> \"what you learned\"  notes are append-only and travel with the ticket
              board new \"title\" --found-in <id>   work you discovered, linked back, never silently done
4. finish     board done <id> \"summary\"           → the next lane (review); someone else reviews
   or hand off board handoff <id> \"note\" --to <who>
   or release board release <id> \"reason\"         back to the queue, nothing recorded as done
5. blocked?   board dep <id> --on <other>           readiness follows `blocks` dependencies
              board set <id> --version V --state blocked   a person moves tickets by hand

Rules the store enforces, not you: a stale fence is refused (your lease lapsed; claim again);
the reviewer must differ from the author; a lapsed lease goes back to the queue, three lapses
send the ticket to a person; WIP is limited; states change only through these verbs.
Exit codes: 0 ok · 2 conflict · 3 refused · 4 not found · 5 board unavailable · 64 usage.";
