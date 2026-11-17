//! `board scan [PATH] [--import]` (PATH: a folder or one file): preview what the scanner finds (`crate::scan`), each with
//! the ticket it already is when one carries its fingerprint; `--import` adds the rest. The
//! same on a local board and on a hub's board, since both go through the face.

use std::path::Path;

use serde::Serialize;

use super::{CommandResult, EXIT_OK, Output, Parsed, no_id, usage_error};
use crate::{
    Actor, Anchors, Filter, NewTicket,
    face::Face,
    scan::{self, Finding},
};

/// Every ticket a scan makes carries this label, beside its rule's.
pub const SCANNED: &str = "scanned";

#[derive(Serialize)]
struct Item<'a> {
    #[serde(flatten)]
    finding: &'a Finding,
    /// The ticket that carries this finding: found on the board, or made by this import.
    ticket: Option<String>,
    imported: bool,
}

#[derive(Serialize)]
struct Report<'a> {
    files: usize,
    truncated: bool,
    found: usize,
    new: usize,
    on_board: usize,
    imported: usize,
    items: Vec<Item<'a>>,
}

pub(super) fn run(
    parsed: &Parsed,
    board: &dyn Face,
    actor: &Actor<'_>,
    board_dir: &Path,
    output: &mut Output<'_>,
) -> CommandResult {
    let project = board_dir
        .parent()
        .ok_or_else(|| usage_error("the board has no project folder"))?;
    let asked = match parsed.positional.first() {
        Some(path) => project.join(path),
        None => project.to_path_buf(),
    };
    let outside = || usage_error(format!("{} is not in this project", asked.display()));
    let project = project.canonicalize().map_err(|_| outside())?;
    let start = asked.canonicalize().map_err(|_| outside())?;
    if !start.starts_with(&project) {
        return Err(outside());
    }
    let rules = no_id(scan::rules_for(board_dir))?;
    let found = scan::scan(&project, &start, &rules);
    let import = parsed.switch("--import");
    let mut items = Vec::with_capacity(found.findings.len());
    for finding in &found.findings {
        let filter = Filter {
            anchors: Anchors {
                fingerprint: Some(finding.fingerprint.clone()),
                ..Anchors::default()
            },
            limit: Some(1),
            ..Filter::default()
        };
        let existing = no_id(board.list(actor, &filter))?.into_iter().next();
        let (ticket, imported) = match existing {
            Some(ticket) => (Some(ticket.id), false),
            None if import => {
                let ticket = no_id(board.create(actor, new_ticket(finding)))?;
                (Some(ticket.id), true)
            }
            None => (None, false),
        };
        items.push(Item {
            finding,
            ticket,
            imported,
        });
    }
    let imported = items.iter().filter(|i| i.imported).count();
    let on_board = items.iter().filter(|i| i.ticket.is_some()).count() - imported;
    let report = Report {
        files: found.files,
        truncated: found.truncated,
        found: items.len(),
        new: items.len() - on_board - imported,
        on_board,
        imported,
        items,
    };
    output.value(&report, || text(&report, import));
    Ok(EXIT_OK)
}

fn new_ticket(finding: &Finding) -> NewTicket {
    let under = finding
        .heading
        .as_ref()
        .map(|h| format!(", under \"{h}\""))
        .unwrap_or_default();
    let status = finding
        .status
        .as_ref()
        .map(|s| format!(" Its status there: {s}."))
        .unwrap_or_default();
    NewTicket {
        title: finding.title.clone(),
        description: format!(
            "Found by `board scan` ({}) in {} line {}{under}.{status} The text is the file's, copied as it was.",
            finding.rule, finding.path, finding.line
        ),
        labels: vec![SCANNED.to_string(), finding.label.clone()],
        anchors: Anchors {
            fingerprint: Some(finding.fingerprint.clone()),
            ..Anchors::default()
        },
        ..NewTicket::default()
    }
}

fn text(report: &Report<'_>, import: bool) -> String {
    let mut lines: Vec<String> = report
        .items
        .iter()
        .map(|item| {
            let f = item.finding;
            match (&item.ticket, item.imported) {
                (Some(id), true) => format!("+ {id}  {}:{}  {}", f.path, f.line, f.title),
                (Some(id), false) => format!("= {id}  {}:{}  {}", f.path, f.line, f.title),
                (None, _) => format!("+ {}:{}  {}", f.path, f.line, f.title),
            }
        })
        .collect();
    let more = if report.truncated {
        " (the first 1000; scan a folder for the rest)"
    } else {
        ""
    };
    lines.push(if import {
        format!(
            "{} found in {} files{more}: {} imported, {} already on the board.",
            report.found, report.files, report.imported, report.on_board
        )
    } else {
        format!(
            "{} found in {} files{more}: {} new, {} already on the board. `board scan --import` adds the new ones.",
            report.found, report.files, report.new, report.on_board
        )
    });
    lines.join("\n")
}
