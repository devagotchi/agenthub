//! Public source defaults only; never seed the original operator's names or credentials.
use super::{Effect, Rule, Selector};

pub(super) fn rules() -> Vec<Rule> {
    let definitions = [
        (
            "claim-ready",
            "agents claim ready work",
            "agent",
            "task.claim:ready",
            Effect::Allow,
            10,
        ),
        (
            "claim-review",
            "reviewers claim review",
            "role:reviewer",
            "task.claim:review",
            Effect::Allow,
            10,
        ),
        (
            "claim-human",
            "humans claim anything",
            "human",
            "task.claim:*",
            Effect::Allow,
            10,
        ),
        (
            "human-label",
            "human-only label",
            "agent",
            "task.claim:*",
            Effect::Deny,
            100,
        ),
        (
            "create",
            "anyone creates tasks",
            "*",
            "task.create",
            Effect::Allow,
            0,
        ),
        ("edit", "anyone edits", "*", "task.edit", Effect::Allow, 0),
        (
            "edit-held",
            "humans edit held tasks",
            "human",
            "task.edit_held",
            Effect::Allow,
            10,
        ),
        (
            "notes",
            "anyone adds notes",
            "*",
            "note.add:*",
            Effect::Allow,
            0,
        ),
        (
            "complete",
            "holders complete",
            "*",
            "task.complete:*",
            Effect::Allow,
            0,
        ),
        (
            "handoff",
            "holders hand off",
            "*",
            "task.handoff",
            Effect::Allow,
            0,
        ),
        (
            "transition",
            "workflow transitions",
            "*",
            "task.transition:*",
            Effect::Allow,
            0,
        ),
        (
            "human-review",
            "only humans approve review directly",
            "agent",
            "task.transition:review->done",
            Effect::Deny,
            50,
        ),
        (
            "human-cancel",
            "only humans cancel",
            "agent",
            "task.transition:*->cancelled",
            Effect::Deny,
            50,
        ),
        (
            "force-release",
            "admins force-release",
            "role:admin",
            "task.force_release",
            Effect::Allow,
            10,
        ),
        (
            "configure",
            "admins configure",
            "role:admin",
            "admin.*",
            Effect::Allow,
            10,
        ),
    ];
    definitions
        .into_iter()
        .map(|(id, name, subject, action, effect, priority)| Rule {
            id: id.into(),
            name: name.into(),
            subject: subject.into(),
            action: action.into(),
            selector: Selector {
                projects: Vec::new(),
                labels: if id == "human-label" {
                    vec!["human-only".into()]
                } else {
                    Vec::new()
                },
            },
            effect,
            priority,
            enabled: true,
            version: 1,
        })
        .collect()
}
