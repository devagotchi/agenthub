//! What a command prints: JSON when stdout is not a terminal, text when it is, and the ticket table.

use std::{io::Write, path::Path};

use devagotchi_contracts::{BridgeError, BridgeResult};
use serde::Serialize;

use crate::Ticket;

use super::{ErrorBody, ErrorOut, exit_code_for, next_for};

pub(super) struct Output<'a> {
    pub(super) json: bool,
    pub(super) quiet: bool,
    pub(super) out: &'a mut dyn Write,
    pub(super) err: &'a mut dyn Write,
}

impl Output<'_> {
    pub(super) fn value(&mut self, value: &impl Serialize, text: impl FnOnce() -> String) {
        if self.json {
            let _ = writeln!(
                self.out,
                "{}",
                serde_json::to_string(value).unwrap_or_default()
            );
        } else if !self.quiet {
            let _ = writeln!(self.out, "{}", text());
        }
    }

    pub(super) fn lines(&mut self, text: &str) {
        if !self.quiet {
            let _ = writeln!(self.out, "{text}");
        }
    }

    pub(super) fn error(&mut self, error: &BridgeError, id: Option<&str>) -> u8 {
        let next = match (error.code.as_str(), error.message.rsplit_once(": ")) {
            // The store's message ends with the board's environments: name them in `next`.
            ("unknown_environment", Some((_, list))) => {
                format!(
                    "pass --env with one of this board's environments ({list}), or leave it out"
                )
            }
            // The store's message is what is wrong and how to give it: it is the hint.
            ("invalid_settings", _) => {
                format!("{}, then run board settings again", error.message)
            }
            _ => next_for(&error.code, id),
        };
        if self.json {
            let body = ErrorOut {
                error: ErrorBody {
                    code: &error.code,
                    message: &error.message,
                    next: next.clone(),
                },
            };
            let _ = writeln!(
                self.out,
                "{}",
                serde_json::to_string(&body).unwrap_or_default()
            );
        } else {
            let _ = writeln!(
                self.err,
                "error {}: {}\n  next: {}",
                error.code, error.message, next
            );
        }
        exit_code_for(&error.code)
    }
}

/// A prime row's anchor line, after the title on the same row (prime stays under 50 lines).
pub(super) fn anchored(anchors: &Option<String>) -> String {
    anchors
        .as_deref()
        .map(|line| format!("  · {line}"))
        .unwrap_or_default()
}

pub(super) fn table(tickets: &[Ticket]) -> String {
    if tickets.is_empty() {
        return "(none)".into();
    }
    tickets
        .iter()
        .map(|t| {
            format!(
                "{:<14} {:<16} P{} {:<10} {}",
                t.id,
                t.state,
                t.priority,
                t.holder.as_deref().unwrap_or("-"),
                trim(&t.title, 70)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn trim(text: &str, max: usize) -> String {
    let one_line = text.lines().next().unwrap_or("");
    if one_line.chars().count() <= max {
        one_line.to_string()
    } else {
        format!("{}…", one_line.chars().take(max - 1).collect::<String>())
    }
}

/// A board time (ms since the epoch) as UTC to the second, `2026-11-18T17:05:09Z`: the
/// proleptic Gregorian calendar from day numbers (H. Hinnant's `civil_from_days`).
pub(super) fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, second) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second / 3600,
        second % 3600 / 60,
        second % 60
    )
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> BridgeResult<()> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| BridgeError::new("board_io", e.to_string()))?;
    std::fs::write(path, text + "\n")
        .map_err(|e| BridgeError::new("board_io", format!("{}: {e}", path.display())))
}
