//! The board's commands, one function each, grouped by what they act on. `dispatch` parses
//! the shared `--limit` and hands the command its context.

use std::path::Path;

use devagotchi_contracts::BridgeError;

use crate::{Actor, face::Face};

use super::{ClaimCache, CommandResult, Output, Parsed, usage_error};

mod leases;
mod reads;
mod scan;
mod tickets;
mod upkeep;

/// What a command works with: the parsed line, the board, who acts, this actor's claim
/// cache, the board folder, where it prints, and the `--limit` (default 20).
pub(super) struct Command<'a, 'o> {
    parsed: &'a Parsed,
    board: &'a dyn Face,
    actor: &'a Actor<'a>,
    cache: &'a mut ClaimCache,
    board_dir: &'a Path,
    output: &'a mut Output<'o>,
    limit: usize,
}

pub(super) fn dispatch(
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
    let c = Command {
        parsed,
        board,
        actor,
        cache,
        board_dir,
        output,
        limit,
    };
    match parsed.command.as_str() {
        "prime" => reads::prime(c),
        "ready" => reads::ready(c),
        "held" => reads::held(c),
        "hooks" => upkeep::hooks(c),
        "ls" => reads::ls(c),
        "show" => reads::show(c),
        "scan" => scan::scan(c),
        "new" => tickets::new(c),
        "claim" => leases::claim(c),
        "beat" if parsed.positional.first().is_none() => leases::beat_all(c),
        "beat" => leases::beat(c),
        "note" => leases::note(c),
        "done" => leases::done(c),
        "handoff" => leases::handoff(c),
        "release" => leases::release(c),
        "force" => leases::force(c),
        "set" => tickets::set(c),
        "dep" => tickets::dep(c),
        "reap" => upkeep::reap(c),
        "verify" => upkeep::verify(c),
        "settings" => upkeep::settings(c),
        "export" => upkeep::export(c),
        other => Err(usage_error(format!("unknown command {other}"))),
    }
}

/// The `n`th positional argument.
fn arg(parsed: &Parsed, n: usize) -> Option<&str> {
    parsed.positional.get(n).map(String::as_str)
}

/// The `n`th positional argument, or a usage error naming `what`.
fn need<'p>(
    parsed: &'p Parsed,
    n: usize,
    what: &str,
) -> Result<&'p str, (BridgeError, Option<String>)> {
    arg(parsed, n).ok_or_else(|| usage_error(format!("{what} is required")))
}

/// Prepared requests already froze the cached fence, including its absence.
fn optional_fence(
    parsed: &Parsed,
    cache: &ClaimCache,
    id: &str,
) -> Result<Option<i64>, (BridgeError, Option<String>)> {
    Ok(parsed.int("--fence").map_err(usage_error)?.or_else(|| {
        if parsed.flag("--request-id").is_some() {
            None
        } else {
            cache.claims.get(id).map(|c| c.fence)
        }
    }))
}

/// `--fence`, or the original cached fence this actor supplied for this request.
fn fence_for(
    parsed: &Parsed,
    cache: &ClaimCache,
    id: &str,
) -> Result<i64, (BridgeError, Option<String>)> {
    optional_fence(parsed, cache, id)?.ok_or_else(|| {
        (
            BridgeError::new(
                "stale_fence",
                format!("no cached claim for {id}; pass --fence or claim it first"),
            ),
            Some(id.to_string()),
        )
    })
}
