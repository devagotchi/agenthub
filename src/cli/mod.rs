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
//! Two faces behind one command line (`face.rs`): a project's local store, or — once
//! `board bind` has named a hub in `.devagotchi/config.json` and an account is on file —
//! the hub's board as that account (B11: the hub arbitrates; the agent's name is its seat
//! there). Every command and every hook runs the same either way.

use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};

use devagotchi_contracts::{BridgeError, BridgeResult};

use crate::{
    Actor, AnchorPatch, BOARD_DIR, DepKind, Filter, NewTicket, NextFilter, Patch, ProjectConfig,
    default_prefix,
    face::{Face, ME},
    open_project, read_project_config,
};

use bind::{bind, remote_face, unbind};
use remote::Remote;

mod args;
mod bind;
mod claims;
mod errors;
mod hooks;
mod hub;
mod init;
mod output;
mod remote;
mod scan;
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
use errors::{
    CommandResult, ErrorBody, ErrorOut, exit_code_for, next_for, no_id, usage_error, with_id,
};
use init::{LOCAL_GITIGNORE, init};
use output::{Output, anchored, table, trim, write_json};

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
        // A project is wherever `.devagotchi/config.json` is: a local store (`init`) or a
        // binding to a hub (`bind`) — both write it.
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
            || (!invocation.stdout_is_terminal && !parsed.switch("--text")));
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
    // The face: the hub's board as the account when bound, else the local store.
    let face: Box<dyn Face> = match &config.hub {
        Some(hub) => match remote_face(hub, &config, &invocation, &actor) {
            Ok(remote) => Box::new(remote),
            Err(error) => return output.error(&error, None),
        },
        None => match open_project(&project_dir) {
            Ok((_, board)) => Box::new(board),
            Err(error) => return output.error(&error, None),
        },
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

fn dispatch(
    parsed: &Parsed,
    board: &dyn Face,
    actor: &Actor<'_>,
    cache: &mut ClaimCache,
    board_dir: &Path,
    output: &mut Output<'_>,
) -> CommandResult {
    let limit = parsed
        .int("--limit")
        .map_err(usage_error)?
        .unwrap_or(20)
        .max(1) as usize;
    let arg = |n: usize| parsed.positional.get(n).map(String::as_str);
    let need =
        |n: usize, what: &str| arg(n).ok_or_else(|| usage_error(format!("{what} is required")));
    let fence_for = |id: &str| -> Result<i64, (BridgeError, Option<String>)> {
        match parsed.int("--fence").map_err(usage_error)? {
            Some(f) => Ok(f),
            None => cache.claims.get(id).map(|c| c.fence).ok_or_else(|| {
                (
                    BridgeError::new(
                        "stale_fence",
                        format!("no cached claim for {id}; pass --fence or claim it first"),
                    ),
                    Some(id.to_string()),
                )
            }),
        }
    };
    match parsed.command.as_str() {
        "prime" => {
            let prime = no_id(board.prime(actor, limit))?;
            output.value(&prime, || {
                let mut lines = vec![format!(
                    "board {} · you are {}{}",
                    prime.project,
                    prime.actor,
                    if prime.human { " (person)" } else { "" }
                )];
                if prime.held.is_empty() {
                    lines.push("held by you: none".into());
                } else {
                    lines.push(format!("held by you ({}): finish or hand off before you stop", prime.held.len()));
                    for t in prime.held.iter().take(10) {
                        let left = t.lease_expires_in_ms.max(0) / 60_000;
                        lines.push(format!("  {}  fence {}  lease {}m left  {}{}", t.id, t.fence, left, trim(&t.title, 60), anchored(&t.anchors)));
                    }
                }
                if prime.ready.is_empty() {
                    lines.push("ready for you: nothing".into());
                } else {
                    lines.push(format!("ready for you ({}): board claim <id>, or board claim for the first", prime.ready.len()));
                    for t in &prime.ready {
                        lines.push(format!("  {}  P{}  {}{}", t.id, t.priority, trim(&t.title, 60), anchored(&t.anchors)));
                    }
                }
                if !prime.attention.is_empty() {
                    lines.push(format!("needs a person ({}):", prime.attention.len()));
                    for t in &prime.attention {
                        lines.push(format!("  {}  {}{}", t.id, trim(&t.title, 60), anchored(&t.anchors)));
                    }
                }
                if !prime.notes_to_me.is_empty() {
                    lines.push(format!("notes to you ({}):", prime.notes_to_me.len()));
                    for note in prime.notes_to_me.iter().take(5) {
                        lines.push(format!("  {}  from {}: {}", note.ticket, note.from, trim(&note.body, 80)));
                    }
                }
                lines.push(format!(
                    "protocol: {} Heartbeat at least every {} min. `board agent` for the whole workflow.",
                    PROTOCOL_LINE,
                    prime.heartbeat_every_ms / 60_000
                ));
                lines.join("\n")
            });
            Ok(EXIT_OK)
        }
        "ready" => {
            let ready = no_id(board.ready(actor, limit))?;
            output.value(&ready, || table(&ready));
            Ok(EXIT_OK)
        }
        "held" => {
            let held = no_id(board.list(
                actor,
                &Filter {
                    holder: Some(ME.to_string()),
                    ..Filter::default()
                },
            ))?;
            let now = board.now();
            output.value(&held, || {
                held.iter()
                    .map(|t| {
                        let left = t
                            .lease_expires_at
                            .map(|e| (e - now).max(0) / 60_000)
                            .unwrap_or(0);
                        format!(
                            "{}  fence {}  {}m left  {}",
                            t.id,
                            t.fence,
                            left,
                            trim(&t.title, 60)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            Ok(EXIT_OK)
        }
        "hooks" => {
            let targets = parsed.flag("--for").unwrap_or("all");
            if !matches!(targets, "claude-code" | "codex" | "all") {
                return Err(usage_error("--for must be claude-code, codex or all"));
            }
            let dir = board_dir.join("hooks");
            std::fs::create_dir_all(&dir)
                .map_err(|e| (BridgeError::new("board_io", e.to_string()), None))?;
            // Keep old launch paths working; generated client config calls the binary directly.
            let gitignore = board_dir.join(".gitignore");
            let current = match std::fs::read_to_string(&gitignore) {
                Ok(current) => current,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err((BridgeError::new("board_io", error.to_string()), None)),
            };
            if !current.lines().any(|l| l == HOOK_MARKER_IGNORE) {
                let mut next = current;
                if !next.is_empty() && !next.ends_with('\n') {
                    next.push('\n');
                }
                next.push_str(HOOK_MARKER_IGNORE);
                next.push('\n');
                std::fs::write(&gitignore, next)
                    .map_err(|error| (BridgeError::new("board_io", error.to_string()), None))?;
            }
            for (name, body) in HOOK_SCRIPTS {
                let path = dir.join(name);
                std::fs::write(&path, body).map_err(|e| {
                    (
                        BridgeError::new("board_io", format!("{}: {e}", path.display())),
                        None,
                    )
                })?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                        .map_err(|error| (BridgeError::new("board_io", error.to_string()), None))?;
                }
            }
            let snippets: Vec<(&str, String)> = [
                ("claude-code", claude_code_hooks()),
                ("codex", codex_hooks()),
            ]
            .into_iter()
            .filter(|(name, _)| targets == "all" || targets == *name)
            .collect();
            output.value(
                &serde_json::json!({
                    "hooks_dir": dir,
                    "scripts": HOOK_SCRIPTS.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
                    "claude_code": snippets.iter().find(|(n, _)| *n == "claude-code").map(|(_, s)| serde_json::from_str::<serde_json::Value>(s).unwrap_or_default()),
                    "codex": snippets.iter().find(|(n, _)| *n == "codex").map(|(_, s)| serde_json::from_str::<serde_json::Value>(s).unwrap_or_default()),
                }),
                || {
                    let mut lines = vec![format!(
                        "wrote {}/{{session-start,beat,stop}}.sh compatibility launchers; config below calls native hooks on Windows and Unix",
                        dir.display()
                    )];
                    for (name, snippet) in &snippets {
                        lines.push(String::new());
                        lines.push(match *name {
                            "claude-code" => "Claude Code — add to .claude/settings.json (project) or ~/.claude/settings.json; the actor name is the last argument:".into(),
                            _ => "Codex — merge into .codex/hooks.json; trust the project and review exact definitions with /hooks. Changed hooks require review again:".into(),
                        });
                        lines.push(snippet.clone());
                    }
                    lines.push(String::new());
                    lines.push("Keep unrelated hooks. The binary must be on PATH. First Stop blocks while holding; repeated Stop/SessionEnd reports interrupted work without looping or completing/releasing another session's leases. No universal enforcement or real-client validation is implied.".into());
                    lines.join("\n")
                },
            );
            Ok(EXIT_OK)
        }
        "ls" => {
            let list = no_id(board.list(
                actor,
                &Filter {
                    state: parsed.flag("--state").map(str::to_string),
                    label: parsed.flag("--label").map(str::to_string),
                    text: parsed.flag("--text").map(str::to_string),
                    open_only: parsed.switch("--open"),
                    anchors: parsed.anchors(),
                    limit: Some(limit),
                    ..Filter::default()
                },
            ))?;
            output.value(&list, || table(&list));
            Ok(EXIT_OK)
        }
        "show" => {
            let id = need(0, "ID")?;
            let view = with_id(id, board.get(id))?;
            output.value(&view, || {
                let t = &view.ticket;
                let mut lines = vec![
                    format!("{}  [{}]  P{}  {}", t.id, t.state, t.priority, t.title),
                    format!(
                        "  kind {} · labels {} · holder {} · assignee {} · version {} · fence {}",
                        t.kind,
                        if t.labels.is_empty() {
                            "-".into()
                        } else {
                            t.labels.join(",")
                        },
                        t.holder.as_deref().unwrap_or("-"),
                        t.assignee.as_deref().unwrap_or("-"),
                        t.version,
                        t.fence
                    ),
                ];
                if let Some(anchors) = t.anchor_line() {
                    lines.push(format!("  {anchors}"));
                }
                if !t.blocked_by.is_empty() {
                    lines.push(format!("  blocked by {}", t.blocked_by.join(", ")));
                }
                if !t.description.is_empty() {
                    lines.push(format!("  {}", t.description.replace('\n', "\n  ")));
                }
                for d in &view.deps {
                    lines.push(format!("  dep {} {}", d.kind.as_str(), d.depends_on));
                }
                for c in &view.children {
                    lines.push(format!("  child {c}"));
                }
                for n in &view.notes {
                    lines.push(format!(
                        "  note #{} {} by {}{}: {}",
                        n.seq,
                        n.kind,
                        n.author,
                        n.to_principal
                            .as_ref()
                            .map(|p| format!(" → {p}"))
                            .unwrap_or_default(),
                        trim(&n.body, 200)
                    ));
                }
                lines.join("\n")
            });
            Ok(EXIT_OK)
        }
        "scan" => scan::run(parsed, board, actor, board_dir, output),
        "new" => {
            let title = need(0, "TITLE")?.to_string();
            let ticket = no_id(board.create(
                actor,
                NewTicket {
                    title,
                    description: parsed.flag("--desc").unwrap_or("").to_string(),
                    kind: parsed.switch("--epic").then(|| "epic".to_string()),
                    priority: parsed.int("--priority").map_err(usage_error)?,
                    labels: parsed.flag_all("--label"),
                    parent: parsed.flag("--parent").map(str::to_string),
                    assignee: parsed.flag("--assign").map(str::to_string),
                    discovered_from: parsed.flag("--found-in").map(str::to_string),
                    anchors: parsed.anchors(),
                    ..NewTicket::default()
                },
            ))?;
            output.value(&ticket, || format!("{}  {}", ticket.id, ticket.title));
            Ok(EXIT_OK)
        }
        "claim" => {
            let ttl = parsed.int("--ttl").map_err(usage_error)?.map(|s| s * 1000);
            let lease = match arg(0) {
                Some(id) => with_id(id, board.claim(actor, id, ttl))?,
                None => {
                    let filter = NextFilter {
                        label: parsed.flag("--label").map(str::to_string),
                        states: parsed.flag_all("--state"),
                        parent: parsed.flag("--parent").map(str::to_string),
                        ttl_ms: ttl,
                    };
                    match no_id(board.claim_next(actor, &filter))? {
                        Some(lease) => lease,
                        None => {
                            output.value(&serde_json::json!({ "claimed": false }), || {
                                "nothing ready for you".into()
                            });
                            return Ok(EXIT_OK);
                        }
                    }
                }
            };
            cache.claims.insert(
                lease.ticket.id.clone(),
                CachedClaim {
                    fence: lease.fence,
                    version: lease.ticket.version,
                    lease_expires_at: lease.lease_expires_at,
                },
            );
            output.value(&lease, || {
                format!(
                    "{}  claimed (fence {}, heartbeat every {}s): {}\n  board beat {} · board done {} \"summary\" · board handoff {} \"note\"",
                    lease.ticket.id,
                    lease.fence,
                    lease.heartbeat_every_ms / 1000,
                    lease.ticket.title,
                    lease.ticket.id,
                    lease.ticket.id,
                    lease.ticket.id
                )
            });
            Ok(EXIT_OK)
        }
        "beat" if arg(0).is_none() => {
            // Every ticket this actor holds, with the fence the store shows (the holder's
            // fence is by definition the current one). What the hook pack calls after tools.
            let held = no_id(board.list(
                actor,
                &Filter {
                    holder: Some(ME.to_string()),
                    ..Filter::default()
                },
            ))?;
            let mut beaten = Vec::new();
            for ticket in &held {
                let lease = with_id(
                    &ticket.id,
                    board.heartbeat(actor, &ticket.id, ticket.fence, None),
                )?;
                cache.claims.insert(
                    ticket.id.clone(),
                    CachedClaim {
                        fence: lease.fence,
                        version: lease.ticket.version,
                        lease_expires_at: lease.lease_expires_at,
                    },
                );
                beaten.push(lease);
            }
            output.value(&beaten, || {
                if beaten.is_empty() {
                    "holding nothing".into()
                } else {
                    beaten
                        .iter()
                        .map(|l| {
                            format!("{}  lease extended to {}", l.ticket.id, l.lease_expires_at)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            });
            Ok(EXIT_OK)
        }
        "beat" => {
            let id = need(0, "ID")?;
            let fence = fence_for(id)?;
            let progress = match parsed.flag("--progress") {
                Some(raw) => Some(
                    serde_json::from_str(raw)
                        .map_err(|_| usage_error("--progress must be JSON"))?,
                ),
                None => None,
            };
            let lease = with_id(id, board.heartbeat(actor, id, fence, progress))?;
            cache.claims.insert(
                id.to_string(),
                CachedClaim {
                    fence: lease.fence,
                    version: lease.ticket.version,
                    lease_expires_at: lease.lease_expires_at,
                },
            );
            output.value(&lease, || {
                format!("{id}  lease extended to {}", lease.lease_expires_at)
            });
            Ok(EXIT_OK)
        }
        "note" => {
            let id = need(0, "ID")?;
            let text = need(1, "TEXT")?;
            let fence = parsed
                .int("--fence")
                .map_err(usage_error)?
                .or_else(|| cache.claims.get(id).map(|c| c.fence));
            let note = with_id(
                id,
                board.add_note(
                    actor,
                    id,
                    parsed.flag("--kind").unwrap_or("note"),
                    text,
                    parsed.flag("--to"),
                    fence,
                ),
            )?;
            output.value(&note, || format!("{id}  note #{}", note.seq));
            Ok(EXIT_OK)
        }
        "done" => {
            let id = need(0, "ID")?;
            let fence = fence_for(id)?;
            let ticket = with_id(id, board.complete(actor, id, fence, arg(1).unwrap_or("")))?;
            cache.claims.remove(id);
            output.value(&ticket, || format!("{id}  → {}", ticket.state));
            Ok(EXIT_OK)
        }
        "handoff" => {
            let id = need(0, "ID")?;
            let note = need(1, "NOTE")?;
            let fence = fence_for(id)?;
            let ticket = with_id(
                id,
                board.handoff(actor, id, fence, note, parsed.flag("--to")),
            )?;
            cache.claims.remove(id);
            output.value(&ticket, || {
                format!(
                    "{id}  → {} (assigned to {})",
                    ticket.state,
                    ticket.assignee.as_deref().unwrap_or("anyone")
                )
            });
            Ok(EXIT_OK)
        }
        "release" => {
            let id = need(0, "ID")?;
            let fence = fence_for(id)?;
            let ticket = with_id(id, board.release(actor, id, fence, arg(1).unwrap_or("")))?;
            cache.claims.remove(id);
            output.value(&ticket, || format!("{id}  → {}", ticket.state));
            Ok(EXIT_OK)
        }
        "force" => {
            let id = need(0, "ID")?;
            let reason = need(1, "REASON")?;
            let ticket = with_id(id, board.force_release(actor, id, reason))?;
            output.value(&ticket, || {
                format!("{id}  → {} (lease taken away)", ticket.state)
            });
            Ok(EXIT_OK)
        }
        "set" => {
            let id = need(0, "ID")?;
            let version = parsed
                .int("--version")
                .map_err(usage_error)?
                .ok_or_else(|| usage_error("--version V is required (board show ID --json)"))?;
            let patch = Patch {
                title: parsed.flag("--title").map(str::to_string),
                description: parsed.flag("--desc").map(str::to_string),
                priority: parsed.int("--priority").map_err(usage_error)?,
                labels: parsed.flags.get("--label").cloned(),
                assignee: parsed
                    .flag("--assign")
                    .map(|a| if a == "-" { None } else { Some(a.to_string()) }),
                state: parsed.flag("--state").map(str::to_string),
                anchors: {
                    let a = parsed.anchors();
                    AnchorPatch {
                        machine: a.machine.map(Some),
                        repo: a.repo.map(Some),
                        environment: a.environment.map(Some),
                        agent: a.agent.map(Some),
                        session: a.session.map(Some),
                        fingerprint: a.fingerprint.map(Some),
                    }
                },
                ..Patch::default()
            };
            let ticket = with_id(id, board.update(actor, id, version, patch))?;
            output.value(&ticket, || {
                format!("{id}  version {} [{}]", ticket.version, ticket.state)
            });
            Ok(EXIT_OK)
        }
        "dep" => {
            let id = need(0, "ID")?;
            let on = parsed
                .flag("--on")
                .ok_or_else(|| usage_error("--on ID is required"))?;
            let kind = match parsed.flag("--kind") {
                None => DepKind::Blocks,
                Some(k) => DepKind::parse(k)
                    .ok_or_else(|| usage_error(format!("unknown dependency kind {k}")))?,
            };
            let deps = if parsed.switch("--remove") {
                with_id(id, board.remove_dep(actor, id, on, kind))?
            } else {
                with_id(id, board.add_dep(actor, id, on, kind))?
            };
            output.value(&deps, || {
                deps.iter()
                    .map(|d| format!("{} {} {}", d.ticket, d.kind.as_str(), d.depends_on))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            Ok(EXIT_OK)
        }
        "reap" => {
            let n = no_id(board.reap())?;
            output.value(&serde_json::json!({ "expired": n }), || {
                format!("{n} lease(s) expired")
            });
            Ok(EXIT_OK)
        }
        "verify" => {
            let status = no_id(board.verify_chain())?;
            output.value(&status, || {
                format!(
                    "{} events · chain {} · head {}",
                    status.entries,
                    if status.ok { "ok" } else { "BROKEN" },
                    &status.head[..12.min(status.head.len())]
                )
            });
            Ok(if status.ok { EXIT_OK } else { EXIT_REFUSED })
        }
        "settings" => {
            let patch = crate::ProjectSettings {
                repo: parsed.flag("--repo").map(str::to_string),
                environments: parsed.flag("--env").map(|list| {
                    list.split(',')
                        .map(str::trim)
                        .filter(|e| !e.is_empty())
                        .map(str::to_string)
                        .collect()
                }),
            };
            let settings = no_id(board.project_settings(actor, &patch))?;
            output.value(&settings, || {
                format!(
                    "repo {} · environments {}",
                    settings.repo.as_deref().unwrap_or("(none)"),
                    settings.environments.join(", ")
                )
            });
            Ok(EXIT_OK)
        }
        "export" => {
            let path = parsed
                .flag("--out")
                .map(PathBuf::from)
                .unwrap_or_else(|| board_dir.join("board.jsonl"));
            let tickets = no_id(board.list(
                actor,
                &Filter {
                    limit: Some(100_000),
                    ..Filter::default()
                },
            ))?;
            let mut text = String::new();
            for ticket in &tickets {
                let view = with_id(&ticket.id, board.get(&ticket.id))?;
                text.push_str(&serde_json::to_string(&view).unwrap_or_default());
                text.push('\n');
            }
            std::fs::write(&path, text)
                .map_err(|e| (BridgeError::new("board_io", e.to_string()), None))?;
            output.value(
                &serde_json::json!({ "path": path, "tickets": tickets.len() }),
                || format!("{} tickets → {}", tickets.len(), path.display()),
            );
            Ok(EXIT_OK)
        }
        other => Err(usage_error(format!("unknown command {other}"))),
    }
}
