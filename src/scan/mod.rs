//! `board scan`: the plans, checklists and TODOs that AI tools and people leave in a project,
//! found by rules that are data. The library (`rules/*.json`, one file per format) knows
//! where Kiro, spec-kit, Cursor and Claude Code keep their plans, the usual plan files,
//! Markdown checklists anywhere, and TODO/FIXME markers in code. A project adds or overrides
//! rules in `.devagotchi/scan.json`; its rules are tried first.
//!
//! A finding's fingerprint is its file and its normalised text (plus which repeat it is), never
//! its line, so edits around it do not make it new and a second scan imports nothing twice.
//! What a file says is data: titles are copied, never followed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use devagotchi_contracts::{BridgeError, BridgeResult};

mod markdown;
mod marker;
#[cfg(test)]
mod tests;

/// The rule library, most specific first: the first rule that matches a file reads it.
pub const LIBRARY: &[(&str, &str)] = &[
    ("skip", include_str!("rules/skip.json")),
    ("kiro", include_str!("rules/kiro.json")),
    ("spec-kit", include_str!("rules/spec-kit.json")),
    ("cursor", include_str!("rules/cursor.json")),
    ("claude", include_str!("rules/claude.json")),
    ("ai-plans", include_str!("rules/ai-plans.json")),
    ("plan-files", include_str!("rules/plan-files.json")),
    ("markdown", include_str!("rules/markdown.json")),
    ("code", include_str!("rules/code.json")),
];

/// The project's own rules, beside its board.
pub const PROJECT_RULES: &str = "scan.json";

/// Folders never walked: build output, dependencies, the board itself.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".devagotchi",
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".turbo",
    "coverage",
];
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_FINDINGS: usize = 1000;
const MAX_TITLE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    /// Added to every ticket the rule finds, beside `scanned`.
    pub label: String,
    /// Path prefixes under the project, with forward slashes. None: anywhere outside hidden
    /// folders. A hidden folder is walked only when a rule names it here.
    #[serde(default)]
    pub under: Vec<String>,
    /// File names matched exactly.
    #[serde(default)]
    pub names: Vec<String>,
    /// Extensions, lowercase, without the dot.
    #[serde(default)]
    pub ext: Vec<String>,
    pub kind: Kind,
    /// For `marker`: the words that open a comment worth a ticket (`TODO`, `FIXME`).
    #[serde(default)]
    pub markers: Vec<String>,
    /// For `markdown`: which table rows are open work. None: tables are not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<TableRule>,
}

/// How a plan's table says a row is open work. Header names and values match in any case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableRule {
    /// The column that says where a row stands (`State`, `Status`).
    pub status: Vec<String>,
    /// Values that make a row open; a cell that starts with one counts (`partial: …`).
    pub open: Vec<String>,
    /// The column a ticket's title comes from (`Task`, `Feature`).
    pub title: Vec<String>,
    /// A column whose value goes before the title (`#`, `Milestone`).
    #[serde(default)]
    pub id: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Markdown: open checkboxes (`- [ ] text`) and, with `table`, rows whose status is open,
    /// each under its nearest heading.
    Markdown,
    /// A comment that starts with a marker and a colon: `// TODO: text`, `# FIXME(ann): text`.
    Marker,
    /// Never read: templates and changelogs, whose open items are no one's work.
    Skip,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub fingerprint: String,
    pub rule: String,
    pub label: String,
    /// Relative to the project, forward slashes.
    pub path: String,
    pub line: usize,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    /// A table row's status cell, as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// One item a reader found in a file.
struct Item {
    line: usize,
    title: String,
    heading: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub findings: Vec<Finding>,
    pub files: usize,
    /// More than the cap were found; the rest wait for the next scan.
    pub truncated: bool,
}

/// The library's rules, in order.
pub fn library() -> Vec<Rule> {
    LIBRARY
        .iter()
        .flat_map(|(name, text)| {
            serde_json::from_str::<Vec<Rule>>(text)
                .unwrap_or_else(|error| panic!("rule file {name}: {error}"))
        })
        .collect()
}

/// The project's rules from `board_dir/scan.json`, then the library's. No file: the library.
pub fn rules_for(board_dir: &Path) -> BridgeResult<Vec<Rule>> {
    let path = board_dir.join(PROJECT_RULES);
    let mut rules = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Vec<Rule>>(&text).map_err(|error| {
            BridgeError::new("invalid_scan_rules", format!("{}: {error}", path.display()))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(BridgeError::new("board_io", error.to_string())),
    };
    for rule in &rules {
        check(rule).map_err(|error| {
            BridgeError::new("invalid_scan_rules", format!("rule {}: {error}", rule.name))
        })?;
    }
    rules.extend(library());
    Ok(rules)
}

/// What a rule must say to find anything.
pub fn check(rule: &Rule) -> Result<(), &'static str> {
    if rule.names.is_empty() && rule.ext.is_empty() {
        return Err("names no file (names or ext)");
    }
    match (rule.kind, &rule.table) {
        (Kind::Marker, _) if rule.markers.is_empty() => Err("a marker rule names its markers"),
        (Kind::Marker | Kind::Skip, Some(_)) => Err("a table is read by a markdown rule"),
        (_, Some(t)) if t.status.is_empty() || t.open.is_empty() || t.title.is_empty() => {
            Err("a table names its status, open and title")
        }
        _ => Ok(()),
    }
}

/// Scan `start`, a folder or one file inside `project`, with `rules`. Paths in findings are
/// relative to `project`, so a part finds the same fingerprints as the whole.
pub fn scan(project: &Path, start: &Path, rules: &[Rule]) -> Scan {
    let mut out = Scan::default();
    let mut repeats = std::collections::HashMap::<(String, String), usize>::new();
    'files: for (path, rel) in files(project, start, rules) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(rule) = rules.iter().find(|rule| applies(rule, &rel, &name)) else {
            continue;
        };
        if rule.kind == Kind::Skip {
            continue;
        }
        if !std::fs::metadata(&path).is_ok_and(|m| m.len() <= MAX_FILE_BYTES) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        out.files += 1;
        for item in read(rule, &text) {
            if out.findings.len() == MAX_FINDINGS {
                out.truncated = true;
                break 'files;
            }
            let key = (rel.clone(), normalise(&item.title));
            let n = repeats.entry(key.clone()).or_default();
            let fingerprint = fingerprint(&key.0, &key.1, *n);
            *n += 1;
            out.findings.push(Finding {
                fingerprint,
                rule: rule.name.clone(),
                label: rule.label.clone(),
                path: rel.clone(),
                line: item.line,
                title: item.title,
                heading: item.heading,
                status: item.status,
            });
        }
    }
    out.findings
        .sort_by(|a, b| (a.path.as_str(), a.line).cmp(&(b.path.as_str(), b.line)));
    out
}

/// The files under `start` (or `start` itself), with their paths from `project`, depth first
/// in name order. Links are never followed: they can leave the project or loop.
fn files(project: &Path, start: &Path, rules: &[Rule]) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    if start.is_file() {
        found.extend(relative(project, start).map(|rel| (start.to_path_buf(), rel)));
        return found;
    }
    let mut stack = vec![start.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        let mut subdirs = Vec::new();
        for entry in entries {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let Some(rel) = relative(project, &path) else {
                continue;
            };
            if kind.is_dir() {
                if walk_into(&entry.file_name().to_string_lossy(), &rel, rules) {
                    subdirs.push(path);
                }
            } else if kind.is_file() {
                found.push((path, rel));
            }
        }
        stack.extend(subdirs.into_iter().rev());
    }
    found
}

/// `scan:` and 32 hex characters of SHA-256 over the file, the text and its repeat number.
pub fn fingerprint(path: &str, normalised_title: &str, repeat: usize) -> String {
    let digest = Sha256::digest(format!("{path}\n{normalised_title}\n{repeat}").as_bytes());
    format!("scan:{}", hex::encode(&digest[..16]))
}

fn normalise(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn relative(project: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(project).ok()?;
    let parts: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

fn hidden(rel: &str) -> bool {
    rel.split('/').any(|part| part.starts_with('.'))
}

fn walk_into(name: &str, rel: &str, rules: &[Rule]) -> bool {
    if SKIP_DIRS.contains(&name) {
        return false;
    }
    if !hidden(rel) {
        return true;
    }
    let dir = format!("{rel}/");
    // Only a rule that reads opens a hidden folder.
    rules
        .iter()
        .filter(|rule| rule.kind != Kind::Skip)
        .flat_map(|rule| &rule.under)
        .any(|under| under.starts_with(&dir) || dir.starts_with(under.as_str()))
}

fn applies(rule: &Rule, rel: &str, name: &str) -> bool {
    // A skip rule with no folders skips its files anywhere, hidden folders too.
    let placed = if rule.under.is_empty() {
        rule.kind == Kind::Skip || !hidden(rel)
    } else {
        rule.under
            .iter()
            .any(|under| rel.starts_with(under.as_str()))
    };
    if !placed {
        return false;
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    rule.names.iter().any(|n| n == name) || ext.is_some_and(|ext| rule.ext.contains(&ext))
}

fn read(rule: &Rule, text: &str) -> Vec<Item> {
    match rule.kind {
        Kind::Markdown => markdown::read(text, rule.table.as_ref()),
        Kind::Skip => Vec::new(),
        Kind::Marker => text
            .lines()
            .enumerate()
            .filter_map(|(i, line)| {
                marker::read(line, &rule.markers).map(|title| Item {
                    line: i + 1,
                    title,
                    heading: None,
                    status: None,
                })
            })
            .collect(),
    }
}

fn clip(text: &str) -> String {
    match text.char_indices().nth(MAX_TITLE_CHARS) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}
