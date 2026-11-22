//! Versioned ticket metadata and workflow edits, including retry of the original result.
use super::edits::{unheld_edit, update_fence};
use super::*;
use crate::{Patch, chain::actions};

#[derive(serde::Serialize, serde::Deserialize)]
struct AuthorizedEdit {
    ticket: Ticket,
    // Recheck the actions that were authorized originally, with today's resource labels.
    // The current state cannot reconstruct the original transition after it succeeded.
    actions: Vec<String>,
}

impl HostedBoard {
    pub async fn update(
        &self,
        actor: &Actor<'_>,
        id: &str,
        version: i64,
        patch: &Patch,
    ) -> BridgeResult<Ticket> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let current = self.ticket(connection, id, &workflow, now).await?;
            self.authorize(connection, actor, "task.edit", &current.labels)
                .await?;
            let receipt: Receipt<AuthorizedEdit> = self
                .begin_mutation(connection, actor, "update_task")
                .await?;
            if let Receipt::Replay(saved) = receipt {
                for action in &saved.actions {
                    self.authorize(connection, actor, action, &current.labels)
                        .await?;
                }
                return Ok(saved.ticket);
            }
            unheld_edit(&current, version)?;
            let mut authorized = Vec::new();
            let mut changed = serde_json::Map::new();
            let mut ticket = current.clone();
            if let Some(progress) = &patch.progress {
                ticket.progress = Some(progress.clone());
                changed.insert("progress".into(), true.into());
            }
            if let Some(title) = &patch.title {
                let title = title.trim();
                if title.is_empty() || title.chars().count() > 300 {
                    return Err(invalid("title must be 1 to 300 characters").into());
                }
                ticket.title = title.into();
                changed.insert("title".into(), serde_json::json!(title));
            }
            if let Some(description) = &patch.description {
                ticket.description = description.clone();
                changed.insert("description".into(), true.into());
            }
            if let Some(priority) = patch.priority {
                ticket.priority = priority;
                changed.insert("priority".into(), priority.into());
            }
            if let Some(labels) = &patch.labels {
                ticket.labels = clean_labels(labels);
                changed.insert("labels".into(), serde_json::json!(ticket.labels));
            }
            if let Some(assignee) = &patch.assignee {
                ticket.assignee = assignee.clone();
                changed.insert("assignee".into(), serde_json::json!(assignee));
            }
            for (column, value) in patch.anchors.columns() {
                if let Some(value) = value {
                    let value = clean_anchor(value);
                    if column == "environment" {
                        let settings: Settings = self.configuration(connection, "settings").await?;
                        check_environment(&settings, value.as_deref())?;
                    }
                    match column {
                        "machine" => ticket.machine = value.clone(),
                        "repo" => ticket.repo = value.clone(),
                        "environment" => ticket.environment = value.clone(),
                        "agent" => ticket.agent = value.clone(),
                        "session" => ticket.session = value.clone(),
                        "fingerprint" => ticket.fingerprint = value.clone(),
                        _ => unreachable!("fixed anchor columns"),
                    }
                    changed.insert(column.into(), serde_json::json!(value));
                }
            }
            if let Some(occurrences) = patch.occurrences {
                if occurrences < 1 {
                    return Err(invalid("occurrences must be at least 1").into());
                }
                ticket.occurrences = occurrences;
                changed.insert("occurrences".into(), occurrences.into());
            }
            if let Some(last_seen_at) = patch.last_seen_at {
                ticket.last_seen_at = last_seen_at;
                changed.insert("last_seen_at".into(), serde_json::json!(last_seen_at));
            }
            if let Some(to) = patch.state.as_ref().filter(|to| *to != &current.state) {
                authorized.push(format!("task.transition:{}->{to}", current.state));
                self.authorize(
                    connection,
                    actor,
                    &format!("task.transition:{}->{to}", current.state),
                    &current.labels,
                )
                .await?;
                crate::check_transition(&workflow, &current, to, actor.id)?;
                ticket.state = to.clone();
                ticket.closed_at = workflow.is_done(to).then_some(now);
                changed.insert("from".into(), serde_json::json!(current.state));
                changed.insert("to".into(), serde_json::json!(to));
            }
            update_fence(&current, actor, patch.fence)?;
            if current.holder.is_some()
                && !current.lease_expired
                && current.holder.as_deref() != Some(actor.id)
            {
                authorized.push("task.edit_held".into());
                self.authorize(connection, actor, "task.edit_held", &current.labels)
                    .await?;
            }
            if changed.is_empty() {
                return receipt
                    .complete(
                        connection,
                        AuthorizedEdit {
                            ticket: current,
                            actions: authorized,
                        },
                    )
                    .await
                    .map(|saved| saved.ticket);
            }
            sqlx::query(
                "UPDATE board_tickets SET title=$4,description=$5,priority=$6,labels=$7,assignee=$8,
                 machine=$9,repo=$10,environment=$11,agent=$12,session=$13,fingerprint=$14,occurrences=$15,
                 last_seen_at=$16,state=$17,closed_at=$18,version=version+1,updated_at=$19,progress=$20
                 WHERE tenant_id=$1 AND board_id=$2 AND id=$3",
            ).bind(&self.tenant).bind(&self.project).bind(id).bind(&ticket.title).bind(&ticket.description)
                .bind(ticket.priority).bind(serde_json::to_string(&ticket.labels).expect("labels json"))
                .bind(&ticket.assignee).bind(&ticket.machine).bind(&ticket.repo).bind(&ticket.environment)
                .bind(&ticket.agent).bind(&ticket.session).bind(&ticket.fingerprint).bind(ticket.occurrences)
                .bind(ticket.last_seen_at).bind(&ticket.state).bind(ticket.closed_at).bind(now)
                .bind(ticket.progress.as_ref().map(serde_json::Value::to_string))
                .execute(&mut *connection).await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::TICKET_UPDATED,
                Some(id),
                patch.fence,
                changed.into(),
            )
            .await?;
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            receipt
                .complete(
                    connection,
                    AuthorizedEdit {
                        ticket,
                        actions: authorized,
                    },
                )
                .await
                .map(|saved| saved.ticket)
        })
    }
}
