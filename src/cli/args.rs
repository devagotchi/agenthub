//! The command line's grammar: the usage text, which flags take a value, and the parse.

use std::{collections::BTreeMap, ffi::OsString};

use crate::Anchors;

pub const USAGE: &str = "Usage: devagotchi board <command> [options]

  init      [--project NAME] [--prefix P] [--storage URL] [--agents-md]   make the board and .devagotchi/ here; --agents-md appends the AGENTS.md block
  bind      [--hub URL] [--project NAME] [--shared]   bind your account's board, or attach a granted configured shared board
  unbind                           back to the standalone board (the hub keeps its board)
  prime     [--limit N]            what to do now: held, ready, notes to you, the protocol (≤ 50 lines)
  onboard                          print the AGENTS.md block
  agent                            print the full workflow for an agent
  ready     [--limit N]            tickets you could claim now, best first
  ls        [--state S] [--label L] [--text T] [--open] [--limit N] [ANCHORS]
  show      ID
  new       TITLE [--desc D] [--priority N] [--label L]... [--parent ID] [--depends-on ID]... [--assign ID] [--found-in ID] [--epic] [ANCHORS]
            [--origin JSON]        imported from {path,item[,revision][,content_sha256]}: once per board; a repeat returns the ticket, changed inputs conflict
  claim     [ID] [--ttl SECONDS] [--label L] [--state S]   no ID: the best ready ticket
  beat      [ID] [--progress JSON] [--fence N]   no ID: every ticket you hold (what the hook pack calls)
  held                             the tickets you hold (empty output when none)
  note      ID TEXT [--kind K] [--to ID] [--fence N]
  done      ID [SUMMARY] [--fence N]
  handoff   ID NOTE [--to ID] [--fence N]
  release   ID [REASON] [--fence N]
  force     ID REASON              a person takes a lease away
  set       ID --version V [--fence N] [--progress JSON] [--title T] [--desc D] [--priority N] [--state S] [--assign ID|-] [ANCHORS]   an anchor of \"\" clears it
  dep       ID --on ID [--kind blocks|parent|related|discovered-from|duplicates|supersedes] [--remove]
  reap                             expire lapsed leases
  verify                           recompute the event chain
  settings  [--repo URL] [--env a,b,c]   the project's repo (https) and environments (1-20 lowercase names); none given: show them
  export    [--out PATH]           board.jsonl (export, never the truth)
  scan      [PATH] [--import]      the plans, checklists and TODOs here (PATH: a folder or a file) as tickets: a preview; --import adds the new ones
  hooks     [--for claude-code|codex|all]   write compatibility launchers and print native lifecycle config (review/trust in the client)
  hook      session-start|beat|stop|session-end   native client hook; bounded JSON on stdin, explicit failures
            [--as auto] the agent is DEVAGOTCHI_ACTOR, else the plugin's client (claude, codex, grok)
            [--if-board] outside a project with a board, answer {} and exit 0 (what the plugin runs)

ANCHORS: --machine M --repo URL --env E --agent A --session S --fingerprint F   where a ticket belongs; on ls, filters
Global: --dir PATH (project directory; default: walk up from here)  --as ID (act as this agent)
        --json (also on when stdout is not a terminal)  --human (force the human form)  --quiet
        --text is a human-output alias except after ls, where --text QUERY searches tasks.
Standalone board: in the Postgres DEVAGOTCHI_BOARD_STORAGE_URL names, else the project's (init --storage URL, no password in it).
Bound project: every command above runs on the hub as the signed-in account; --as / DEVAGOTCHI_ACTOR is your seat there.
Exit codes: 0 ok · 2 conflict (fence, version, held, blocked, WIP, source) · 3 refused · 4 not found · 5 board unavailable · 64 usage";

pub(super) struct Parsed {
    pub(super) command: String,
    pub(super) positional: Vec<String>,
    pub(super) flags: BTreeMap<String, Vec<String>>,
    pub(super) switches: Vec<String>,
}

pub(super) const VALUE_FLAGS: &[&str] = &[
    "--dir",
    "--as",
    "--project",
    "--prefix",
    "--storage",
    "--hub",
    "--limit",
    "--state",
    "--label",
    "--text",
    "--desc",
    "--priority",
    "--parent",
    "--depends-on",
    "--assign",
    "--found-in",
    "--origin",
    "--ttl",
    "--progress",
    "--fence",
    "--kind",
    "--to",
    "--version",
    "--title",
    "--on",
    "--out",
    "--for",
    "--machine",
    "--repo",
    "--env",
    "--agent",
    "--session",
    "--fingerprint",
];

pub(super) const SWITCHES: &[&str] = &[
    "--json",
    "--human",
    "--text",
    "--quiet",
    "--agents-md",
    "--epic",
    "--open",
    "--remove",
    "--shared",
    "--if-board",
    "--import",
];

pub(super) fn parse(args: &[OsString]) -> Result<Parsed, String> {
    let mut command = None;
    let mut positional = Vec::new();
    let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut switches = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let arg = arg.to_string_lossy().into_owned();
        if arg == "--help" || arg == "-h" {
            // Help wins wherever it appears: `board --help`, `board claim --help`.
            command = Some("help".into());
            continue;
        }
        if let Some(stripped) = arg.strip_prefix("--") {
            let name = format!("--{}", stripped.split('=').next().unwrap_or(""));
            let search = name == "--text" && command.as_deref() == Some("ls");
            if SWITCHES.contains(&name.as_str()) && !search {
                switches.push(name);
            } else if VALUE_FLAGS.contains(&name.as_str()) {
                let value = match arg.split_once('=') {
                    Some((_, v)) => v.to_string(),
                    None => iter
                        .next()
                        .map(|v| v.to_string_lossy().into_owned())
                        .ok_or_else(|| format!("{name} needs a value"))?,
                };
                flags.entry(name).or_default().push(value);
            } else {
                return Err(format!("unknown option {name}"));
            }
        } else if command.is_none() {
            command = Some(arg);
        } else {
            positional.push(arg);
        }
    }
    Ok(Parsed {
        command: command.unwrap_or_else(|| "help".into()),
        positional,
        flags,
        switches,
    })
}

impl Parsed {
    pub(super) fn flag(&self, name: &str) -> Option<&str> {
        self.flags
            .get(name)
            .and_then(|v| v.last())
            .map(String::as_str)
    }
    pub(super) fn flag_all(&self, name: &str) -> Vec<String> {
        self.flags.get(name).cloned().unwrap_or_default()
    }
    pub(super) fn switch(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }
    /// The anchor flags, as given.
    pub(super) fn anchors(&self) -> Anchors {
        let get = |name: &str| self.flag(name).map(str::to_string);
        Anchors {
            machine: get("--machine"),
            repo: get("--repo"),
            environment: get("--env"),
            agent: get("--agent"),
            session: get("--session"),
            fingerprint: get("--fingerprint"),
        }
    }
    pub(super) fn int(&self, name: &str) -> Result<Option<i64>, String> {
        match self.flag(name) {
            None => Ok(None),
            Some(v) => v
                .parse::<i64>()
                .map(Some)
                .map_err(|_| format!("{name} must be an integer")),
        }
    }
}
