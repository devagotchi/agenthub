//! Commands on the board itself: hooks, reap, verify, settings and export.

use std::path::PathBuf;

use devagotchi_contracts::BridgeError;

use crate::Filter;
use crate::cli::{
    CommandResult, EXIT_OK, EXIT_REFUSED, HOOK_MARKER_IGNORE, HOOK_SCRIPTS, claude_code_hooks,
    codex_hooks, no_id, usage_error, with_id,
};

use super::Command;

/// `board hooks`: write the hook launchers and print the client hook configs.
pub(super) fn hooks(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        actor: _,
        board_dir,
        output,
        ..
    } = c;
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

/// `board reap`: expire lapsed leases.
pub(super) fn reap(c: Command<'_, '_>) -> CommandResult {
    let Command { board, output, .. } = c;
    let n = no_id(board.reap())?;
    output.value(&serde_json::json!({ "expired": n }), || {
        format!("{n} lease(s) expired")
    });
    Ok(EXIT_OK)
}

/// `board verify`: check the board's hash chain.
pub(super) fn verify(c: Command<'_, '_>) -> CommandResult {
    let Command { board, output, .. } = c;
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

/// `board settings`: set the project's repository and environments.
pub(super) fn settings(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        output,
        ..
    } = c;
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

/// `board export`: write every ticket as JSON lines.
pub(super) fn export(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        board_dir,
        output,
        limit: _,
        ..
    } = c;
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
    std::fs::write(&path, text).map_err(|e| (BridgeError::new("board_io", e.to_string()), None))?;
    output.value(
        &serde_json::json!({ "path": path, "tickets": tickets.len() }),
        || format!("{} tickets → {}", tickets.len(), path.display()),
    );
    Ok(EXIT_OK)
}
