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
        | "wip_limit" | "blocked" | "distinct_required" | "dependency_cycle" | "not_held"
        | "source_conflict" | "parent_closed" | "request_conflict" | "already_exists"
        | "in_use" | "shared_workflow" | "not_completable" => EXIT_CONFLICT,
        "not_found" => EXIT_NOT_FOUND,
        "board_store" | "board_random" | "board_id" | "no_board" | "no_storage"
        | "board_runtime" | "board_io" | "hub_unreachable" | "hub_error" => EXIT_UNAVAILABLE,
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
        "shared_workflow" => "edit the shared workflow through its owner, or detach the board before editing its private workflow".into(),
        "in_use" => "detach every board listed as using this workflow before deleting it".into(),
        "already_exists" => "use the existing shared workflow or choose a different workflow id".into(),
        "not_available" => "use an owner-scoped hub board for the shared workflow catalog".into(),
        "wip_limit" => "board done / handoff / release one of the tickets you hold first".into(),
        "source_conflict" => {
            "board show the ticket the message names and reconcile it with board set; never change the origin's path or item to get past it".into()
        }
        "blocked" => format!("board show {id} — finish or hand off the blockers it lists"),
        "distinct_required" => "someone else must take this lane; leave it for them".into(),
        "not_claimable" => format!(
            "board show {id} — only a lane queue is claimable; a person moves it with board set"
        ),
        "not_completable" => format!(
            "board show {id}; ask the board owner to repair its active workflow stage before retrying completion"
        ),
        "use_lease_ops" => {
            "use claim / done / handoff / release; held tickets do not move by set".into()
        }
        "transition_not_allowed" => "board agent lists the transitions the workflow allows".into(),
        "policy_denied" => "ask the environment owner to review this agent's roles and matching policy rule; changing credentials does not change its authority".into(),
        "humans_only" => {
            "ask the person who owns this board; run without --as to act as yourself".into()
        }
        "not_owner" => {
            "only the account that owns this board, or a person granted on it, changes its settings; ask them".into()
        }
        "not_found" => "board ls --open to see the ids on this board".into(),
        "no_board" => "run board init in the project directory, or pass --dir".into(),
        "no_storage" => {
            "set DEVAGOTCHI_BOARD_STORAGE_URL to a postgres:// URL (or board init --storage URL, no password in it), or board bind --hub URL".into()
        }
        "invalid_storage" => {
            "name the Postgres without a password: PGPASSWORD or ~/.pgpass holds it, or set DEVAGOTCHI_BOARD_STORAGE_URL".into()
        }
        "invalid" => "fix the argument the message names and run it again".into(),
        "invalid_settings" => {
            "board settings shows this board's settings; fix the value the message names".into()
        }
        "unknown_environment" => {
            "board settings lists this board's environments: use one, or add it with board settings --env".into()
        }
        "parent_closed" => {
            "board new without --parent, or reopen the parent with board set".into()
        }
        "dependency_cycle" => {
            format!("board show {id} and the other ticket: one already depends on the other; board dep ... --remove the old link first")
        }
        "not_held" => format!("board show {id}: nobody holds it, so there is nothing to release"),
        "approval_decide_only" => {
            "an approval changes only by a person's decision on the hub's approvals page".into()
        }
        "request_conflict" => {
            "retry with the same content, or use a new request id for new content".into()
        }
        "read_only" => "you are a viewer on this board; ask its owner for a member role".into(),
        "board_damaged" => {
            "the board's stored data is damaged; tell whoever runs its Postgres (board verify shows the chain)".into()
        }
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
