//! The hooks a project installs: the launch scripts and the Claude Code, Codex and plugin hook configs.

/// Legacy Unix launch paths delegate all behavior and error handling to the native command.
pub const HOOK_MARKER_IGNORE: &str = "hooks/.last-beat-*";

pub const HOOK_SCRIPTS: &[(&str, &str)] = &[
    (
        "session-start.sh",
        "#!/bin/sh
exec \"${DEVAGOTCHI:-devagotchi}\" board hook session-start --dir \"$(dirname \"$0\")/../..\" --as \"${1:-${DEVAGOTCHI_ACTOR:-claude}}\"
",
    ),
    (
        "beat.sh",
        "#!/bin/sh
exec \"${DEVAGOTCHI:-devagotchi}\" board hook beat --dir \"$(dirname \"$0\")/../..\" --as \"${1:-${DEVAGOTCHI_ACTOR:-claude}}\"
",
    ),
    (
        "stop.sh",
        "#!/bin/sh
exec \"${DEVAGOTCHI:-devagotchi}\" board hook stop --dir \"$(dirname \"$0\")/../..\" --as \"${1:-${DEVAGOTCHI_ACTOR:-claude}}\"
",
    ),
];

fn hook_command(phase: &str, actor: &str) -> String {
    format!("devagotchi board hook {phase} --as {actor}")
}

/// Claude Code's `hooks` block: SessionStart stdout is added to the context, PostToolUse
/// runs after every tool, Stop exit 2 blocks the stop with stderr shown to the model.
pub fn claude_code_hooks() -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "hooks": {
            "SessionStart": [{ "hooks": [{ "type": "command", "command": hook_command("session-start", "claude"), "timeout": 30 }] }],
            "PostToolUse": [{ "matcher": "", "hooks": [{ "type": "command", "command": hook_command("beat", "claude"), "timeout": 30 }] }],
            "Stop": [{ "hooks": [{ "type": "command", "command": hook_command("stop", "claude"), "timeout": 30 }] }],
            "SessionEnd": [{ "hooks": [{ "type": "command", "command": hook_command("session-end", "claude"), "timeout": 30 }] }]
        }
    }))
    .expect("hooks json")
}

/// Codex's current hooks.json schema; project and exact definitions must be trusted.
pub fn codex_hooks() -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "hooks": {
            "SessionStart": [{ "hooks": [{ "type": "command", "command": hook_command("session-start", "codex"), "timeout": 30 }] }],
            "PostToolUse": [{ "matcher": "", "hooks": [{ "type": "command", "command": hook_command("beat", "codex"), "timeout": 30 }] }],
            "Stop": [{ "hooks": [{ "type": "command", "command": hook_command("stop", "codex"), "timeout": 30 }] }],
            "SessionEnd": [{ "hooks": [{ "type": "command", "command": hook_command("session-end", "codex"), "timeout": 30 }] }]
        }
    }))
    .expect("hooks json")
}

/// The hooks the `devagotchi-agenthub` plugin ships (`plugins/devagotchi-agenthub/hooks/hooks.json`,
/// held to this by a test): the same four as [`claude_code_hooks`], for whichever client runs
/// the plugin (`--as auto`), and silent in a project with no board (`--if-board`).
pub fn plugin_hooks() -> String {
    let command = |phase: &str| format!("devagotchi board hook {phase} --as auto --if-board");
    serde_json::to_string_pretty(&serde_json::json!({
        "hooks": {
            "SessionStart": [{ "hooks": [{ "type": "command", "command": command("session-start"), "timeout": 30 }] }],
            "PostToolUse": [{ "matcher": "", "hooks": [{ "type": "command", "command": command("beat"), "timeout": 30 }] }],
            "Stop": [{ "hooks": [{ "type": "command", "command": command("stop"), "timeout": 30 }] }],
            "SessionEnd": [{ "hooks": [{ "type": "command", "command": command("session-end"), "timeout": 30 }] }]
        }
    }))
    .expect("hooks json")
}
