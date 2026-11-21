use super::*;

fn rule(id: &str, subject: &str, action: &str, effect: Effect, priority: i64) -> Rule {
    Rule {
        id: id.into(),
        name: id.into(),
        subject: subject.into(),
        action: action.into(),
        effect,
        priority,
        selector: Selector::default(),
        enabled: true,
        version: 1,
    }
}

#[test]
fn source_policy_priority_selectors_and_deny_ties_hold_in_any_order() {
    let mut rules = vec![
        rule("claim", "agent", "task.claim:*", Effect::Allow, 10),
        rule("label", "agent", "task.claim:*", Effect::Deny, 100),
        rule("tie-deny", "role:reviewer", "review", Effect::Deny, 5),
        rule("tie-allow", "role:reviewer", "review", Effect::Allow, 5),
        rule("disabled", "*", "disabled", Effect::Allow, 100),
    ];
    rules[1].selector.labels = vec!["human-only".into()];
    rules[4].enabled = false;
    let roles = vec!["reviewer".into()];
    let subject = Subject {
        id: "@agent:owner:codex",
        kind: "codex",
        human: false,
        roles: &roles,
    };
    let labels = vec!["human-only".into()];
    for _ in 0..2 {
        assert!(decide(&rules, &subject, "task.claim:ready", &Resource::default()).allowed);
        assert!(
            !decide(
                &rules,
                &subject,
                "task.claim:ready",
                &Resource {
                    project: "alpha",
                    labels: &labels
                }
            )
            .allowed
        );
        let denied = decide(&rules, &subject, "review", &Resource::default());
        assert_eq!(
            denied,
            Decision {
                allowed: false,
                rule: Some("tie-deny".into())
            }
        );
        for action in ["disabled", "unknown"] {
            assert_eq!(
                decide(&rules, &subject, action, &Resource::default()),
                Decision {
                    allowed: false,
                    rule: None
                }
            );
        }
        rules.reverse();
    }
}

#[test]
fn subject_and_all_labels_are_checked_without_treating_names_as_patterns() {
    let roles = vec!["worker".into()];
    let subject = Subject {
        id: "@agent:a:one",
        kind: "codex",
        human: false,
        roles: &roles,
    };
    let labels = vec!["a".into(), "b".into()];
    for (pattern, expected) in [
        ("*", true),
        ("agent", true),
        ("human", false),
        ("role:worker", true),
        ("role:admin", false),
        ("principal:@agent:a:one", true),
        ("principal:@agent:b:one", false),
        ("kind:codex", true),
        ("kind:*", false),
    ] {
        let mut r = rule("check", pattern, "task.*", Effect::Allow, 0);
        r.selector = Selector {
            projects: vec!["alpha".into()],
            labels: labels.clone(),
        };
        let resource = Resource {
            project: "alpha",
            labels: &labels,
        };
        assert_eq!(
            decide(&[r.clone()], &subject, "task.edit", &resource).allowed,
            expected
        );
        assert!(
            !decide(
                &[r.clone()],
                &subject,
                "task.edit",
                &Resource {
                    project: "beta",
                    labels: &labels
                }
            )
            .allowed
        );
        assert!(
            !decide(
                &[r],
                &subject,
                "task.edit",
                &Resource {
                    project: "alpha",
                    labels: &labels[..1]
                }
            )
            .allowed
        );
    }
}

#[test]
fn action_globs_do_not_overlap_literals_or_use_other_pattern_syntax() {
    for (pattern, value, expected) in [
        (
            "task.transition:*->done",
            "task.transition:review->done",
            true,
        ),
        ("admin.*", "task.edit", false),
        ("*", "", true),
        ("", "", true),
        ("abc", "abcd", false),
        ("ab*bc", "abc", false),
        ("ab*bc", "abbc", true),
        ("a**b*c", "a/b:c", true),
        ("*雪*", "白雪山", true),
        ("a?", "ab", false),
    ] {
        assert_eq!(glob(pattern, value), expected, "{pattern} {value}");
    }
}

#[test]
fn malformed_rules_cannot_be_read_as_broader_selectors() {
    let mut valid = rule("safe", "role:worker", "task.*", Effect::Allow, 1);
    assert!(valid.validate().is_ok());
    valid.subject = "role:".into();
    assert!(valid.validate().is_err());
    assert!(serde_json::from_value::<Selector>(serde_json::json!({"project": ["alpha"]})).is_err());
    assert!(serde_json::from_value::<Selector>(serde_json::json!({"labels": null})).is_err());
    assert!(serde_json::from_value::<Effect>(serde_json::json!("ALLOW")).is_err());
}
