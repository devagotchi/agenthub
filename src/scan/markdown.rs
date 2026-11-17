//! Open work in Markdown: unchecked boxes, and rows of a plan's table whose status column says
//! they are not finished. Each item keeps its nearest heading. Fenced code is skipped.

use super::{Item, TableRule, clip};

/// Where the columns a table rule reads sit in the table under way.
struct Columns {
    status: usize,
    title: usize,
    id: Option<usize>,
}

pub(super) fn read(text: &str, table: Option<&TableRule>) -> Vec<Item> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    let mut heading = None;
    let mut fenced = false;
    let mut columns: Option<Columns> = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if !trimmed.starts_with('|') {
            columns = None;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            let text = rest.trim_start_matches('#').trim();
            if !text.is_empty() {
                heading = Some(clip(text));
            }
            continue;
        }
        if let Some(title) = open_item(trimmed) {
            found.push(Item {
                line: i + 1,
                title,
                heading: heading.clone(),
                status: None,
            });
            continue;
        }
        let Some(rule) = table else {
            continue;
        };
        if !trimmed.starts_with('|') {
            continue;
        }
        // A header row is the one right above the `|---|` row.
        if lines.get(i + 1).is_some_and(|next| separator(next)) {
            columns = header(&cells(trimmed), rule);
            continue;
        }
        if separator(trimmed) {
            continue;
        }
        let Some(at) = &columns else {
            continue;
        };
        let row = cells(trimmed);
        let (Some(status), Some(title)) = (row.get(at.status), row.get(at.title)) else {
            continue;
        };
        if title.is_empty() || !open(status, &rule.open) {
            continue;
        }
        let id = at.id.and_then(|n| row.get(n)).filter(|id| !id.is_empty());
        found.push(Item {
            line: i + 1,
            title: clip(&match id {
                Some(id) => format!("{id} {title}"),
                None => title.clone(),
            }),
            heading: heading.clone(),
            status: Some(clip(status)),
        });
    }
    found
}

/// The text of an open checkbox item (`- [ ] text`, `* [ ]`, `+ [ ]`, `1. [ ]`), if this is one.
fn open_item(line: &str) -> Option<String> {
    let rest = if let Some(rest) = line.strip_prefix(['-', '*', '+']) {
        rest
    } else {
        let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return None;
        }
        line[digits..].strip_prefix(['.', ')'])?
    };
    let rest = rest.strip_prefix([' ', '\t'])?.trim_start();
    let title = rest.strip_prefix("[ ]")?.trim();
    (!title.is_empty()).then(|| clip(title))
}

/// A row's cells, trimmed, with bold markers and runs of spaces removed.
fn cells(row: &str) -> Vec<String> {
    let inner = row.trim().trim_start_matches('|');
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner
        .split('|')
        .map(|cell| {
            cell.replace("**", "")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn separator(row: &str) -> bool {
    let row = row.trim();
    row.starts_with('|')
        && row.contains('-')
        && row
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

fn header(names: &[String], rule: &TableRule) -> Option<Columns> {
    let find = |wanted: &[String]| names.iter().position(|name| starts_with_word(name, wanted));
    Some(Columns {
        status: find(&rule.status)?,
        title: find(&rule.title)?,
        id: find(&rule.id),
    })
}

/// The cell starts with an open value, as a whole word: `partial: …` is open, `partially` not.
fn open(status: &str, values: &[String]) -> bool {
    starts_with_word(status.trim_matches(['`', '*', '_']), values)
}

/// `text` starts with one of `words` (any case) and the next character, if any, ends a word:
/// `Feature (short)` is a `feature` column.
fn starts_with_word(text: &str, words: &[String]) -> bool {
    let text = text.to_lowercase();
    words.iter().any(|word| {
        let word = word.to_lowercase();
        text.starts_with(&word)
            && !text[word.len()..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric)
    })
}
