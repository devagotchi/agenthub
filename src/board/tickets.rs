//! Changing tickets: creating one, updating it along the workflow, and adding a note.

use devagotchi_contracts::BridgeResult;
use rusqlite::params;

use crate::{
    Actor, Anchors, Board, DepKind, NewTicket, Note, Patch, Ticket, approval, check_environment,
    check_transition, clean_anchor, clean_labels, fail, insert_dep, insert_note, invalid,
    load_settings, load_workflow, read_note, read_ticket, store_error,
};

impl Board {
    pub fn create(&self, actor: &Actor<'_>, new: NewTicket) -> BridgeResult<Ticket> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.create(actor, &new), true);
        }
        let title = new.title.trim().to_string();
        if title.is_empty() || title.chars().count() > 300 {
            return Err(invalid("title must be 1 to 300 characters"));
        }
        let kind = new.kind.unwrap_or_else(|| "task".into());
        if !matches!(kind.as_str(), "task" | "epic" | "message") {
            return Err(invalid("kind must be task, epic or message"));
        }
        let labels = clean_labels(&new.labels);
        let anchors = Anchors {
            machine: clean_anchor(&new.anchors.machine),
            repo: clean_anchor(&new.anchors.repo),
            environment: clean_anchor(&new.anchors.environment),
            agent: clean_anchor(&new.anchors.agent),
            session: clean_anchor(&new.anchors.session),
            fingerprint: clean_anchor(&new.anchors.fingerprint),
        };
        let occurrences = new.occurrences.unwrap_or(1);
        if occurrences < 1 {
            return Err(invalid("occurrences must be at least 1"));
        }
        self.write(actor, |w| {
        let workflow = load_workflow(w.tx)?;
        let settings = load_settings(w.tx)?;
        check_environment(&settings, anchors.environment.as_deref())?;
        let now = w.now;
        let id = match &new.parent {
            Some(parent) => {
                let parent_row = read_ticket(w.tx, &workflow, parent, now)?;
                if workflow.is_done(&parent_row.state) {
                    return Err(fail("parent_closed", format!("{parent} is closed")));
                }
                let next: i64 = w
                        .tx
                        .query_row(
                            "UPDATE tickets SET child_seq = child_seq + 1 WHERE id = ?1 RETURNING child_seq",
                            params![parent],
                            |row| row.get(0),
                        )
                        .map_err(store_error)?;
                format!("{parent}.{next}")
            }
            None => w.fresh_id(&self.prefix, &self.project, &title, actor.id)?,
        };
        if let Some(from) = &new.discovered_from {
            read_ticket(w.tx, &workflow, from, now)?;
        }
        let priority = new.priority.unwrap_or(settings.default_priority);
        w.tx
                .execute(
                    "INSERT INTO tickets (id, kind, title, description, state, priority, labels, parent,
                       assignee, created_by, created_at, updated_at, machine, repo, environment, agent,
                       session, fingerprint, occurrences, last_seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, ?13, ?14, ?15,
                       ?16, ?17, ?18, ?19)",
                    params![
                        id,
                        kind,
                        title,
                        new.description,
                        workflow.initial_state,
                        priority,
                        serde_json::to_string(&labels).expect("labels json"),
                        new.parent,
                        new.assignee,
                        actor.id,
                        now,
                        anchors.machine,
                        anchors.repo,
                        anchors.environment,
                        anchors.agent,
                        anchors.session,
                        anchors.fingerprint,
                        occurrences,
                        new.last_seen_at,
                    ],
                )
                .map_err(store_error)?;
        if let Some(parent) = &new.parent {
            insert_dep(w.tx, &id, parent, DepKind::Parent, actor.id, now)?;
        }
        if let Some(from) = &new.discovered_from {
            insert_dep(w.tx, &id, from, DepKind::DiscoveredFrom, actor.id, now)?;
        }
        w.event(
                "ticket.created",
                Some(&id),
                None,
                serde_json::json!({ "title": title, "state": workflow.initial_state, "parent": new.parent }),
            )?;
        read_ticket(w.tx, &workflow, &id, now)
        })
    }

    /// Edit fields and, for an unheld ticket, move it along a workflow transition.
    /// `version` must match (optimistic concurrency); held tickets refuse state changes.
    pub fn update(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        patch: Patch,
    ) -> BridgeResult<Ticket> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(hosted.update(actor, id, version, &patch), true);
        }
        self.write(actor, |w| {
            let workflow = load_workflow(w.tx)?;
            let now = w.now;
            let current = read_ticket(w.tx, &workflow, id, now)?;
            if current.kind == approval::KIND {
                return Err(fail(
                    "approval_decide_only",
                    "an approval changes only through a person's decision",
                ));
            }
            if current.version != version {
                return Err(fail(
                    "version_conflict",
                    format!(
                        "version {version} is not current ({}); re-read and retry",
                        current.version
                    ),
                ));
            }
            let mut changed = serde_json::Map::new();
            if let Some(title) = &patch.title {
                let title = title.trim();
                if title.is_empty() || title.chars().count() > 300 {
                    return Err(invalid("title must be 1 to 300 characters"));
                }
                w.tx.execute(
                    "UPDATE tickets SET title = ?2 WHERE id = ?1",
                    params![id, title],
                )
                .map_err(store_error)?;
                changed.insert("title".into(), serde_json::json!(title));
            }
            if let Some(description) = &patch.description {
                w.tx.execute(
                    "UPDATE tickets SET description = ?2 WHERE id = ?1",
                    params![id, description],
                )
                .map_err(store_error)?;
                changed.insert("description".into(), serde_json::json!(true));
            }
            if let Some(priority) = patch.priority {
                w.tx.execute(
                    "UPDATE tickets SET priority = ?2 WHERE id = ?1",
                    params![id, priority],
                )
                .map_err(store_error)?;
                changed.insert("priority".into(), serde_json::json!(priority));
            }
            if let Some(labels) = &patch.labels {
                let labels = clean_labels(labels);
                w.tx.execute(
                    "UPDATE tickets SET labels = ?2 WHERE id = ?1",
                    params![id, serde_json::to_string(&labels).expect("labels json")],
                )
                .map_err(store_error)?;
                changed.insert("labels".into(), serde_json::json!(labels));
            }
            if let Some(assignee) = &patch.assignee {
                w.tx.execute(
                    "UPDATE tickets SET assignee = ?2 WHERE id = ?1",
                    params![id, assignee],
                )
                .map_err(store_error)?;
                changed.insert("assignee".into(), serde_json::json!(assignee));
            }
            for (column, value) in patch.anchors.columns() {
                let Some(value) = value else { continue };
                let value = clean_anchor(value);
                if column == "environment" {
                    check_environment(&load_settings(w.tx)?, value.as_deref())?;
                }
                // `column` is one of six fixed names, never caller text.
                w.tx.execute(
                    &format!("UPDATE tickets SET {column} = ?2 WHERE id = ?1"),
                    params![id, value],
                )
                .map_err(store_error)?;
                changed.insert(column.into(), serde_json::json!(value));
            }
            if let Some(occurrences) = patch.occurrences {
                if occurrences < 1 {
                    return Err(invalid("occurrences must be at least 1"));
                }
                w.tx.execute(
                    "UPDATE tickets SET occurrences = ?2 WHERE id = ?1",
                    params![id, occurrences],
                )
                .map_err(store_error)?;
                changed.insert("occurrences".into(), serde_json::json!(occurrences));
            }
            if let Some(last_seen_at) = patch.last_seen_at {
                w.tx.execute(
                    "UPDATE tickets SET last_seen_at = ?2 WHERE id = ?1",
                    params![id, last_seen_at],
                )
                .map_err(store_error)?;
                changed.insert("last_seen_at".into(), serde_json::json!(last_seen_at));
            }
            if let Some(to) = patch.state.as_ref().filter(|to| *to != &current.state) {
                check_transition(&workflow, &current, to, actor.id)?;
                let closed_at: Option<i64> = workflow.is_done(to).then_some(now);
                w.tx.execute(
                    "UPDATE tickets SET state = ?2, closed_at = ?3 WHERE id = ?1",
                    params![id, to, closed_at],
                )
                .map_err(store_error)?;
                changed.insert("from".into(), serde_json::json!(current.state));
                changed.insert("to".into(), serde_json::json!(to));
            }
            if changed.is_empty() {
                return Ok(current);
            }
            w.tx.execute(
                "UPDATE tickets SET version = version + 1, updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )
            .map_err(store_error)?;
            w.event(
                "ticket.updated",
                Some(id),
                None,
                serde_json::Value::Object(changed),
            )?;
            read_ticket(w.tx, &workflow, id, now)
        })
    }

    /// Append a note. A note on a ticket the author holds carries the fence, which
    /// is checked; anyone may annotate an unheld ticket without one.
    pub fn add_note(
        &self,
        actor: &Actor<'_>,
        id: &str,
        kind: &str,
        body: &str,
        to_principal: Option<&str>,
        fence: Option<i64>,
    ) -> BridgeResult<Note> {
        if let Some(hosted) = &self.hosted {
            return self.hosted_call(
                hosted.add_note(actor, id, kind, body, to_principal, fence),
                true,
            );
        }
        let body = body.trim();
        if body.is_empty() || body.chars().count() > 20_000 {
            return Err(invalid("note body must be 1 to 20000 characters"));
        }
        let kind = if kind.trim().is_empty() {
            "note"
        } else {
            kind.trim()
        };
        self.write(actor, |w| {
            let workflow = load_workflow(w.tx)?;
            let now = w.now;
            let ticket = read_ticket(w.tx, &workflow, id, now)?;
            if ticket.holder.as_deref() == Some(actor.id) && !ticket.lease_expired {
                crate::lease::check_held(&ticket, actor.id, fence)?;
            }
            let seq = insert_note(w.tx, id, actor.id, kind, body, to_principal, fence, now)?;
            w.event(
                "note.added",
                Some(id),
                fence,
                serde_json::json!({ "seq": seq, "kind": kind, "to": to_principal }),
            )?;
            read_note(w.tx, seq)
        })
    }
}
