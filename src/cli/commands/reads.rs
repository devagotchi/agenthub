//! Commands that read the board: prime, ready, held, ls and show.

use crate::cli::{CommandResult, EXIT_OK, PROTOCOL_LINE, anchored, no_id, table, trim, with_id};
use crate::{Filter, face::ME};

use super::{Command, need};

/// `board prime`, what a session runs first: what it holds, what is ready for it, what needs a person, notes to it and the protocol.
pub(super) fn prime(c: Command<'_, '_>) -> CommandResult {
    let Command {
        board,
        actor,
        output,
        limit,
        ..
    } = c;
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
            lines.push(format!(
                "held by you ({}): finish or hand off before you stop",
                prime.held.len()
            ));
            for t in prime.held.iter().take(10) {
                let left = t.lease_expires_in_ms.max(0) / 60_000;
                lines.push(format!(
                    "  {}  fence {}  lease {}m left  {}{}",
                    t.id,
                    t.fence,
                    left,
                    trim(&t.title, 60),
                    anchored(&t.anchors)
                ));
            }
        }
        if prime.ready.is_empty() {
            lines.push("ready for you: nothing".into());
        } else {
            lines.push(format!(
                "ready for you ({}): board claim <id>, or board claim for the first",
                prime.ready.len()
            ));
            for t in &prime.ready {
                lines.push(format!(
                    "  {}  P{}  {}{}",
                    t.id,
                    t.priority,
                    trim(&t.title, 60),
                    anchored(&t.anchors)
                ));
            }
        }
        if !prime.attention.is_empty() {
            lines.push(format!("needs a person ({}):", prime.attention.len()));
            for t in &prime.attention {
                lines.push(format!(
                    "  {}  {}{}",
                    t.id,
                    trim(&t.title, 60),
                    anchored(&t.anchors)
                ));
            }
        }
        if !prime.notes_to_me.is_empty() {
            lines.push(format!("notes to you ({}):", prime.notes_to_me.len()));
            for note in prime.notes_to_me.iter().take(5) {
                lines.push(format!(
                    "  {}  from {}: {}",
                    note.ticket,
                    note.from,
                    trim(&note.body, 80)
                ));
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

/// `board ready`: tickets this actor could claim now.
pub(super) fn ready(c: Command<'_, '_>) -> CommandResult {
    let Command {
        board,
        actor,
        output,
        limit,
        ..
    } = c;
    let ready = no_id(board.ready(actor, limit))?;
    output.value(&ready, || table(&ready));
    Ok(EXIT_OK)
}

/// `board held`: tickets this actor holds, with fence and lease left.
pub(super) fn held(c: Command<'_, '_>) -> CommandResult {
    let Command {
        board,
        actor,
        output,
        ..
    } = c;
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

/// `board ls`: tickets by state, label, text and anchors.
pub(super) fn ls(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        output,
        limit,
        ..
    } = c;
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

/// `board show`: one ticket with its dependencies, children and notes.
pub(super) fn show(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        output,
        ..
    } = c;
    let id = need(parsed, 0, "ID")?;
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
