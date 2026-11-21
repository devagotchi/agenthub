//! Immutable local preparation of CLI retries; PostgreSQL owns the mutation result.
//! No credentials are stored. Files retain intent and resolved nonsecret command inputs.
use super::{ClaimCache, Parsed};
use devagotchi_contracts::{BridgeError, BridgeResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

#[cfg(test)]
mod tests;

const MAX_RECORD: u64 = 1024 * 1024;

pub(super) struct Selection {
    pub key: String,
    pub operation: &'static str,
}

/// Only one mutation can own a key. Batch/hook lifecycle calls have separate protocols.
pub(super) fn select(parsed: &Parsed) -> BridgeResult<Option<Selection>> {
    let operation = match parsed.command.as_str() {
        "new" => Some("create_task"),
        "claim" => Some(if parsed.positional.is_empty() {
            "claim_next"
        } else {
            "claim"
        }),
        "beat" if !parsed.positional.is_empty() => Some("heartbeat"),
        "note" => Some("add_note"),
        "done" => Some("complete"),
        "handoff" => Some("handoff"),
        "release" => Some("release"),
        "force" => Some("force_release"),
        "set" => Some("update_task"),
        "dep" => Some(if parsed.switch("--remove") {
            "remove_dep"
        } else {
            "add_dep"
        }),
        "settings" if parsed.flag("--repo").is_some() || parsed.flag("--env").is_some() => {
            Some("set_project")
        }
        _ => None,
    };
    let Some(operation) = operation else {
        if parsed.flag("--request-id").is_some() {
            return Err(BridgeError::new(
                "usage",
                "--request-id requires one task or settings mutation; reads, batches and hooks do not accept it",
            ));
        }
        return Ok(None);
    };
    let key = match parsed.flag("--request-id") {
        Some(key) => key.to_owned(),
        None => {
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(|_| {
                BridgeError::new("board_random", "cannot generate request identity")
            })?;
            format!("cli-{}", hex::encode(bytes))
        }
    };
    if key.is_empty()
        || key.len() > 200
        || key.chars().any(char::is_control)
        || parsed.flag_all("--request-id").len() > 1
    {
        return Err(BridgeError::new(
            "usage",
            "supply one request key of 1 to 200 bytes without control characters",
        ));
    }
    Ok(Some(Selection { key, operation }))
}

/// Capture before implicit session/fence defaults. Presentation and location are not edits.
pub(super) fn intent(parsed: &Parsed) -> Value {
    let mut flags = parsed.flags.clone();
    for name in ["--request-id", "--dir", "--as", "--hub", "--storage"] {
        flags.remove(name);
    }
    let mut switches: Vec<_> = parsed
        .switches
        .iter()
        .filter(|s| !matches!(s.as_str(), "--json" | "--human" | "--text" | "--quiet"))
        .cloned()
        .collect();
    switches.sort();
    switches.dedup();
    json!({"command": parsed.command, "positional": parsed.positional, "flags": flags, "switches": switches})
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u32,
    namespace: String,
    key: String,
    intent: Value,
    resolved: Parsed,
}

/// The lock stays owned until dispatch ends, including an ambiguous transport failure.
pub(super) struct Prepared {
    pub parsed: Parsed,
    pub input: Value,
    _lock: File,
}

fn io(error: impl std::fmt::Display) -> BridgeError {
    BridgeError::new("board_io", format!("request preparation: {error}"))
}

pub(super) fn prepare(
    board_dir: &Path,
    scope: &Value,
    selection: &Selection,
    original: &Value,
    parsed: &Parsed,
    cache: &ClaimCache,
) -> BridgeResult<Prepared> {
    let namespace = hex::encode(Sha256::digest(serde_json::to_vec(scope).map_err(io)?));
    let name = hex::encode(Sha256::digest(
        serde_json::to_vec(&(&namespace, &selection.key)).map_err(io)?,
    ));
    let directory = board_dir.join("requests");
    fs::create_dir_all(&directory).map_err(io)?;
    // Existing projects also need the privacy rule; append without replacing other rules.
    ignore(board_dir)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("{name}.lock")))
        .map_err(io)?;
    lock.lock().map_err(io)?;
    let path = directory.join(format!("{name}.json"));
    let record = match File::open(&path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(MAX_RECORD + 1)
                .read_to_end(&mut bytes)
                .map_err(io)?;
            if bytes.len() as u64 > MAX_RECORD {
                return Err(io("stored request exceeds its size limit"));
            }
            let saved: Record = serde_json::from_slice(&bytes).map_err(|_| {
                io("stored request is damaged; preserve it and reconcile the original request")
            })?;
            if saved.schema != 1 || saved.namespace != namespace || saved.key != selection.key {
                return Err(io("stored request identity does not match"));
            }
            if saved.intent != *original {
                return Err(BridgeError::new(
                    "request_conflict",
                    "this request key belongs to different CLI input; retry the original invocation",
                ));
            }
            let mut check = intent(&saved.resolved);
            for flag in ["--fence", "--session"] {
                if original["flags"].get(flag).is_none() {
                    check["flags"]
                        .as_object_mut()
                        .expect("intent flags")
                        .remove(flag);
                }
            }
            if check != *original
                || saved.resolved.flag("--request-id") != Some(selection.key.as_str())
            {
                return Err(io("stored request arguments do not match their intent"));
            }
            saved
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut resolved = parsed.clone();
            // These selected the already-open face/actor and are not mutation arguments.
            // In particular a storage URL must never be copied into this journal.
            for flag in ["--dir", "--as", "--hub", "--storage"] {
                resolved.flags.remove(flag);
            }
            // Freeze cached fences before release/done/handoff can remove or replace them.
            if matches!(
                parsed.command.as_str(),
                "beat" | "note" | "done" | "handoff" | "release" | "set"
            ) && parsed.flag("--fence").is_none()
                && let Some(claim) = parsed
                    .positional
                    .first()
                    .and_then(|id| cache.claims.get(id))
            {
                resolved
                    .flags
                    .insert("--fence".into(), vec![claim.fence.to_string()]);
            }
            resolved
                .flags
                .insert("--request-id".into(), vec![selection.key.clone()]);
            let record = Record {
                schema: 1,
                namespace,
                key: selection.key.clone(),
                intent: original.clone(),
                resolved,
            };
            let bytes = serde_json::to_vec(&record).map_err(io)?;
            if bytes.len() as u64 > MAX_RECORD {
                return Err(io("request exceeds its size limit"));
            }
            let temporary = directory.join(format!("{name}.pending"));
            // A crash before rename leaves no dispatched mutation. Replace only that key's
            // incomplete staging file under its lock; never overwrite a completed record.
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temporary)
                .map_err(io)?;
            file.write_all(&bytes).map_err(io)?;
            file.sync_all().map_err(io)?;
            drop(file);
            fs::rename(&temporary, &path).map_err(io)?;
            #[cfg(unix)]
            File::open(&directory)
                .and_then(|directory| directory.sync_all())
                .map_err(io)?;
            record
        }
        Err(error) => return Err(io(error)),
    };
    // The local receipt hashes the complete prepared input; the remote adapter sends its
    // equivalent typed MCP input. Neither identity includes current transport defaults.
    let input = intent(&record.resolved);
    Ok(Prepared {
        parsed: record.resolved,
        input,
        _lock: lock,
    })
}

fn ignore(board_dir: &Path) -> BridgeResult<()> {
    let path = board_dir.join(".gitignore");
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(&path)
        .map_err(io)?;
    file.lock().map_err(io)?;
    let mut text = String::new();
    (&file).take(65537).read_to_string(&mut text).map_err(io)?;
    if text.len() > 65536 {
        return Err(io("local ignore file is too large"));
    }
    if !text.lines().any(|line| line == "requests/") {
        if !text.is_empty() && !text.ends_with('\n') {
            file.write_all(b"\n").map_err(io)?;
        }
        file.write_all(b"requests/\n").map_err(io)?;
        file.sync_all().map_err(io)?;
    }
    Ok(())
}
