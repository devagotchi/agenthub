//! How a command fails: one exit code per failure class, the `next` step every error carries, and the JSON error shape.

use devagotchi_contracts::{BridgeError, BridgeResult};
use serde::Serialize;

pub const EXIT_OK: u8 = 0;

pub const EXIT_CONFLICT: u8 = 2;

pub const EXIT_REFUSED: u8 = 3;

pub const EXIT_NOT_FOUND: u8 = 4;

pub const EXIT_UNAVAILABLE: u8 = 5;

pub const EXIT_USAGE: u8 = 64;

pub(super) fn exit_code_for(code: &str) -> u8 {
    match code {
        "stale_fence" | "version_conflict" | "held_by_other" | "lease_expired" | "not_holder"
        | "wip_limit" | "blocked" | "distinct_required" | "dependency_cycle" | "not_held" => {
            EXIT_CONFLICT
        }
        "not_found" => EXIT_NOT_FOUND,
        "board_store" | "board_open" | "board_mismatch" | "board_version" | "board_random"
        | "board_id" | "no_board" | "board_io" | "hub_unreachable" | "hub_error" => {
            EXIT_UNAVAILABLE
        }
        "usage" => EXIT_USAGE,
        _ => EXIT_REFUSED,
    }
}

pub(super) fn next_for(code: &str, id: Option<&str>) -> String {
    let id = id.unwrap_or("<id>");
    match code {
        "not_signed_in" | "signed_out" => {
            "devagotchi login (this project's board is on the hub); board unbind to work locally".into()
        }
        "hub_unreachable" => "retry when the hub answers; board unbind to work locally".into(),
        "not_offered" | "plan_limit" | "name_taken" => {
            "devagotchi whoami lists your boards there; bind another name, or ask the hub's operator".into()
        }
        "stale_fence" | "lease_expired" | "not_holder" => {
            format!("board show {id} — see who holds it; board claim {id} to take it if it is free")
        }
        "held_by_other" => {
            format!("board show {id}; wait, or board claim with no id for another ticket")
        }
        "version_conflict" => {
            format!("board show {id} --json and retry set with the current version")
        }
        "wip_limit" => "board done / handoff / release one of the tickets you hold first".into(),
        "blocked" => format!("board show {id} — finish or hand off the blockers it lists"),
        "distinct_required" => "someone else must take this lane; leave it for them".into(),
        "not_claimable" => format!(
            "board show {id} — only a lane queue is claimable; a person moves it with board set"
        ),
        "use_lease_ops" => {
            "use claim / done / handoff / release; held tickets do not move by set".into()
        }
        "transition_not_allowed" => "board agent lists the transitions the workflow allows".into(),
        "humans_only" => {
            "ask the person who owns this board; run without --as to act as yourself".into()
        }
        "not_owner" => {
            "only the account that owns this board, or a person granted on it, changes its settings; ask them".into()
        }
        "not_found" => "board ls --open to see the ids on this board".into(),
        "no_board" => "run board init in the project directory, or pass --dir".into(),
        "usage" => "board --help".into(),
        _ => "board --help".into(),
    }
}

#[derive(Serialize)]
pub(super) struct ErrorOut<'a> {
    pub(super) error: ErrorBody<'a>,
}

#[derive(Serialize)]
pub(super) struct ErrorBody<'a> {
    pub(super) code: &'a str,
    pub(super) message: &'a str,
    pub(super) next: String,
}

pub(super) type CommandResult = Result<u8, (BridgeError, Option<String>)>;

pub(super) fn usage_error(message: impl Into<String>) -> (BridgeError, Option<String>) {
    (BridgeError::new("usage", message), None)
}

pub(super) fn with_id<T>(
    id: &str,
    result: BridgeResult<T>,
) -> Result<T, (BridgeError, Option<String>)> {
    result.map_err(|error| (error, Some(id.to_string())))
}

pub(super) fn no_id<T>(result: BridgeResult<T>) -> Result<T, (BridgeError, Option<String>)> {
    result.map_err(|error| (error, None))
}
