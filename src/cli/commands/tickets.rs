//! Commands that create and change tickets: new, set and dep.

use crate::cli::{CommandResult, EXIT_OK, no_id, usage_error, with_id};
use crate::{AnchorPatch, DepKind, NewTicket, Patch};

use super::{Command, need};

/// `board new`: create a ticket.
pub(super) fn new(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        output,
        ..
    } = c;
    let title = need(parsed, 0, "TITLE")?.to_string();
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

/// `board set`: change a ticket at a known version.
pub(super) fn set(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        output,
        ..
    } = c;
    let id = need(parsed, 0, "ID")?;
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

/// `board dep`: add or remove a dependency.
pub(super) fn dep(c: Command<'_, '_>) -> CommandResult {
    let Command {
        parsed,
        board,
        actor,
        output,
        ..
    } = c;
    let id = need(parsed, 0, "ID")?;
    let on = parsed
        .flag("--on")
        .ok_or_else(|| usage_error("--on ID is required"))?;
    let kind = match parsed.flag("--kind") {
        None => DepKind::Blocks,
        Some(k) => {
            DepKind::parse(k).ok_or_else(|| usage_error(format!("unknown dependency kind {k}")))?
        }
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
