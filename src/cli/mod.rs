//! `devagotchi board …`: the agent-facing command line over one project's board.
//! Agent-first by contract (docs/agent-board.md §7): JSON whenever stdout is not a
//! terminal, `--quiet`, one exit code per failure class, every error carries `next`,
//! and `prime` is what a session runs first — the tickets it holds, what is ready for
//! it, and the one-line protocol, in under fifty lines.
//!
//! Identity on this surface is the OS user: the board file is theirs, as the broker's
//! trust scope is (plan §5.4). An agent names itself with `--as` or `DEVAGOTCHI_ACTOR`
//! and is then an agent (no force-release); a bare OS user is the human.
//!
//! Two faces behind one command line (`face.rs`): a project's standalone board in Postgres
//! (`DEVAGOTCHI_BOARD_STORAGE_URL`, `project.rs`), or — once `board bind` has named a hub in
//! `.devagotchi/config.json` and an account is on file — the hub's board as that account (B11:
//! the hub arbitrates; the agent's name is its seat there). Every command and every hook runs
//! the same either way.

use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};

use devagotchi_contracts::{BridgeError, BridgeResult};

use crate::{
    Actor, BOARD_DIR, ProjectConfig, default_prefix, face::Face, open_project, read_project_config,
};

use bind::{bind, remote_face, unbind};
use remote::Remote;

mod args;
mod bind;
mod claims;
mod commands;
mod errors;
mod hooks;
mod hub;
mod init;
mod output;
mod remote;
#[cfg(test)]
mod tests;

pub use args::USAGE;
pub use errors::{
    EXIT_CONFLICT, EXIT_NOT_FOUND, EXIT_OK, EXIT_REFUSED, EXIT_UNAVAILABLE, EXIT_USAGE,
};
pub use hooks::{HOOK_MARKER_IGNORE, HOOK_SCRIPTS, claude_code_hooks, codex_hooks, plugin_hooks};
pub use hub::{Attached, Hub, Session, call};
pub use init::{AGENT_WORKFLOW, AGENTS_MD_END, AGENTS_MD_START, PROTOCOL_LINE, agents_md_block};

use args::{Parsed, parse};
use claims::{CachedClaim, ClaimCache, read_cache, write_cache};
use commands::dispatch;
use errors::{
    CommandResult, ErrorBody, ErrorOut, exit_code_for, next_for, no_id, usage_error, with_id,
};
use init::{LOCAL_GITIGNORE, init};
use output::{Output, anchored, table, trim, utc, write_json};

/// Everything the command line needs from its environment, so tests can supply it.
pub struct Invocation {
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// `DEVAGOTCHI_ACTOR`, if set.
    pub actor_env: Option<String>,
    /// `DEVAGOTCHI_SESSION`, if set: the agent CLI's session id, which the hook pack
    /// exports at session start. A new ticket carries it unless `--session` says otherwise.
    pub session_env: Option<String>,
    /// The OS user (`USER` / `USERNAME`).
    pub os_user: Option<String>,
    /// `DEVAGOTCHI_BOARD_STORAGE_URL`, if set: the Postgres a standalone board is in.
    pub storage_env: Option<String>,
    pub stdout_is_terminal: bool,
    /// `account.json`, for a bound project; none when the environment has no home.
    pub account_file: Option<PathBuf>,
    /// The AI client whose plugin runs this command ([`plugin_host`]): who `--as auto` is
    /// when `DEVAGOTCHI_ACTOR` is not set.
    pub plugin_host: Option<String>,
    /// The hub a bound project is on, reached as the account ([`Hub`]); none, and a bound
    /// project is refused as not signed in.
    pub hub: Option<std::sync::Arc<dyn Hub>>,
}

/// The AI client whose plugin started this process, from the variables each client's plugin
/// loader documents for its hooks: Grok sets `GROK_PLUGIN_ROOT`; Codex sets `PLUGIN_ROOT`
/// (and `CLAUDE_PLUGIN_ROOT` too, for compatibility); Claude Code sets only
/// `CLAUDE_PLUGIN_ROOT`. Checked in that order; none set, none.
pub fn plugin_host(var: impl Fn(&str) -> Option<OsString>) -> Option<String> {
    let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
    if set("GROK_PLUGIN_ROOT") {
        Some("grok".into())
    } else if set("PLUGIN_ROOT") {
        Some("codex".into())
    } else if set("CLAUDE_PLUGIN_ROOT") {
        Some("claude".into())
    } else {
        None
    }
}

/// Walk up from `start` to the first directory holding `.devagotchi/config.json`.
pub fn find_project_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start.to_path_buf());
    while let Some(d) = dir {
        // A project is wherever `.devagotchi/config.json` is: a standalone board (`init`) or
        // a binding to a hub (`bind`) — both write it.
        if d.join(BOARD_DIR).join("config.json").is_file() {
            return Some(d);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
}

/// Run one invocation. Returns the process exit code; writes to `out` and `err`.
pub fn run(mut invocation: Invocation, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let mut parsed = match parse(&invocation.args) {
        Ok(p) => p,
        Err(message) => {
            let _ = writeln!(err, "error usage: {message}\n  next: board --help");
            return EXIT_USAGE;
        }
    };
    if parsed.command == "new"
        && let Some(session) = invocation
            .session_env
            .as_ref()
            .filter(|s| !s.trim().is_empty())
    {
        parsed
            .flags
            .entry("--session".into())
            .or_insert_with(|| vec![session.clone()]);
    }
    let json = parsed.command != "hook"
        && (parsed.switch("--json")
            || (!invocation.stdout_is_terminal
                && !parsed.switch("--text")
                && !parsed.switch("--human")));
    let mut output = Output {
        json,
        quiet: parsed.switch("--quiet"),
        out,
        err,
    };
    if matches!(parsed.command.as_str(), "help" | "--help") {
        let _ = writeln!(output.out, "{USAGE}");
        return EXIT_OK;
    }
    let named = match parsed.flag("--as") {
        Some("auto") => match invocation
            .actor_env
            .clone()
            .or_else(|| invocation.plugin_host.clone())
        {
            Some(id) => Some(id),
            None => {
                return output.error(
                    &BridgeError::new(
                        "usage",
                        "--as auto names the agent from DEVAGOTCHI_ACTOR or the plugin that runs this; neither is set",
                    ),
                    None,
                );
            }
        },
        Some(id) => Some(id.to_string()),
        None => invocation.actor_env.clone(),
    };
    let (actor_id, human) = match named {
        Some(id) => (id, false),
        None => (
            invocation
                .os_user
                .clone()
                .unwrap_or_else(|| "operator".into()),
            true,
        ),
    };
    let actor = Actor {
        id: &actor_id,
        human,
        wip_limit: None,
    };
    let hook_input = if parsed.command == "hook" {
        let input = match hooks::read_input(std::io::stdin().lock()) {
            Ok(input) => input,
            Err(error) => return output.error(&error, None),
        };
        if human {
            return output.error(
                &BridgeError::new("usage", "hooks require --as AGENT or DEVAGOTCHI_ACTOR"),
                None,
            );
        }
        if let Some(cwd) = &input.cwd {
            invocation.cwd = cwd.clone();
        }
        Some(input)
    } else {
        None
    };
    match parsed.command.as_str() {
        "onboard" => {
            output.lines(&agents_md_block());
            return EXIT_OK;
        }
        "agent" => {
            output.lines(AGENT_WORKFLOW);
            return EXIT_OK;
        }
        "init" => return init(&parsed, &invocation, &mut output),
        "bind" => return bind(&parsed, &invocation, &actor, &mut output),
        _ => {}
    }
    let project_dir = match parsed.flag("--dir") {
        Some(dir) => {
            let dir = invocation.cwd.join(dir);
            if dir.join(BOARD_DIR).join("config.json").is_file() {
                dir
            } else {
                return output.error(
                    &BridgeError::new("no_board", format!("no board under {}", dir.display())),
                    None,
                );
            }
        }
        None => match find_project_dir(&invocation.cwd) {
            Some(dir) => dir,
            // A plugin's hooks run in every project; one without a board is not an error.
            None if hook_input.is_some() && parsed.switch("--if-board") => {
                let _ = writeln!(output.out, "{{}}");
                return EXIT_OK;
            }
            None => {
                return output.error(
                    &BridgeError::new("no_board", "no .devagotchi/ here or above"),
                    None,
                );
            }
        },
    };
    let board_dir = project_dir.join(BOARD_DIR);
    if parsed.command == "unbind" {
        return unbind(&board_dir, &mut output);
    }
    let config = match read_project_config(&board_dir) {
        Ok(config) => config,
        Err(error) => return output.error(&error, None),
    };
    // The face: the hub's board as the account when bound, else the standalone board, on a
    // runtime of its own that outlives it (declared first, dropped last).
    let local_runtime;
    let face: Box<dyn Face> = match &config.hub {
        Some(hub) => match remote_face(hub, &config, &invocation, &actor) {
            Ok(remote) => Box::new(remote),
            Err(error) => return output.error(&error, None),
        },
        None => {
            local_runtime = match crate::runtime() {
                Ok(runtime) => runtime,
                Err(error) => return output.error(&error, None),
            };
            match open_project(
                &project_dir,
                invocation.storage_env.as_deref(),
                &local_runtime,
            ) {
                Ok((_, board)) => Box::new(board),
                Err(error) => return output.error(&error, None),
            }
        }
    };
    if let Some(input) = hook_input {
        let env_file = std::env::var_os("CLAUDE_ENV_FILE").map(PathBuf::from);
        return hooks::run(
            face.as_ref(),
            &actor,
            parsed.positional.first().map(String::as_str).unwrap_or(""),
            &input,
            env_file.as_deref(),
            &mut output,
        );
    }
    let mut cache = read_cache(&board_dir, &actor_id);
    let result = dispatch(
        &parsed,
        face.as_ref(),
        &actor,
        &mut cache,
        &board_dir,
        &mut output,
    );
    write_cache(&board_dir, &actor_id, &cache);
    match result {
        Ok(code) => code,
        Err((error, id)) => output.error(&error, id.as_deref()),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
