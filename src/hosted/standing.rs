//! Standing authorizations in Postgres: one exact step a person approved ahead of time on a
//! board, kept in its settings, granted, revoked and suggested on the database's clock
//! (`crate::approval` says what one covers). Each change is one transaction, one event each.

use super::*;
use crate::approval::{
    DAY_MS, KIND, MAX_LIVE, MAX_STANDING_DAYS, SUGGEST_AFTER, Standing, Suggestion, Verdict,
    checked, label_value, landing, parts, text,
};
use crate::chain::actions;

impl HostedBoard {
    /// A person approves the exact command `what` of `kind` (on `target`) ahead of time on
    /// this board for `days` (1 to `MAX_STANDING_DAYS`).
    pub async fn grant_standing(
        &self,
        actor: &Actor<'_>,
        kind: &str,
        target: Option<&str>,
        what: &str,
        days: u32,
    ) -> BridgeResult<Standing> {
        if !actor.human {
            return Err(fail(
                "humans_only",
                "only a person approves a step ahead of time",
            ));
        }
        let kind = checked("kind", kind)?;
        let target = target.map(|t| checked("target", t)).transpose()?;
        let what = text("what", what, 4000, true)?;
        let days = checked_days(days)?;
        let id = standing_id();
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let made = Standing {
                id: id.clone(),
                kind: kind.clone(),
                target: target.clone(),
                what: what.clone(),
                granted_by: actor.id.to_string(),
                granted_at: now,
                expires_at: now + i64::from(days) * DAY_MS,
                revoked_at: None,
                revoked_by: None,
            };
            self.grant_in(connection, actor, now, made).await
        })
    }

    /// Add `made` to the board's standing authorizations in the caller's transaction, after
    /// forgetting what ended over 30 days ago (the chain keeps it); `too_many` at the limit.
    pub(super) async fn grant_in(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        now: i64,
        made: Standing,
    ) -> DbResult<Standing> {
        let mut settings: Settings = self.configuration(connection, "settings").await?;
        settings.standing.retain(|s| {
            s.live(now)
                || now - s.revoked_at.unwrap_or(s.expires_at).min(s.expires_at) < 30 * DAY_MS
        });
        if settings.standing.iter().filter(|s| s.live(now)).count() >= MAX_LIVE {
            return Err(fail(
                "too_many",
                format!(
                    "this board already approves {MAX_LIVE} steps ahead of time; revoke one first"
                ),
            )
            .into());
        }
        settings.standing.push(made.clone());
        self.save_configuration(connection, "settings", &settings)
            .await?;
        self.event(
            connection,
            actor.id,
            now,
            actions::APPROVAL_STANDING_GRANTED,
            None,
            None,
            serde_json::json!({
                "id": made.id, "kind": made.kind, "target": made.target, "what": made.what,
                "expires_at": made.expires_at,
            }),
        )
        .await?;
        Ok(made)
    }

    /// A person ends a standing authorization at once.
    pub async fn revoke_standing(&self, actor: &Actor<'_>, id: &str) -> BridgeResult<Standing> {
        if !actor.human {
            return Err(fail(
                "humans_only",
                "only a person ends an approval given ahead of time",
            ));
        }
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let ended = self
                .revoke_in(connection, actor, now, &|s: &Standing| s.id == id)
                .await?;
            ended.into_iter().next().ok_or_else(|| {
                fail(
                    "not_found",
                    format!("no live standing authorization {id} here"),
                )
                .into()
            })
        })
    }

    /// End every live standing authorization `actor` granted for one of `kinds`: what a person
    /// who now wants those kinds always asked takes back. What ended, possibly nothing.
    pub async fn revoke_own(
        &self,
        actor: &Actor<'_>,
        kinds: &[String],
    ) -> BridgeResult<Vec<Standing>> {
        if !actor.human {
            return Err(fail(
                "humans_only",
                "only a person ends an approval given ahead of time",
            ));
        }
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            self.revoke_in(connection, actor, now, &|s: &Standing| {
                s.granted_by == actor.id && kinds.contains(&s.kind)
            })
            .await
        })
    }

    /// End the live standing authorizations `which` picks, one `approval.standing_revoked`
    /// each; none picked writes nothing.
    async fn revoke_in(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        now: i64,
        which: &dyn Fn(&Standing) -> bool,
    ) -> DbResult<Vec<Standing>> {
        let mut settings: Settings = self.configuration(connection, "settings").await?;
        let mut ended = Vec::new();
        for standing in settings.standing.iter_mut() {
            if standing.live(now) && which(standing) {
                standing.revoked_at = Some(now);
                standing.revoked_by = Some(actor.id.to_string());
                ended.push(standing.clone());
            }
        }
        if ended.is_empty() {
            return Ok(ended);
        }
        self.save_configuration(connection, "settings", &settings)
            .await?;
        for standing in &ended {
            self.event(
                connection,
                actor.id,
                now,
                actions::APPROVAL_STANDING_REVOKED,
                None,
                None,
                serde_json::json!({ "id": standing.id, "kind": standing.kind, "target": standing.target }),
            )
            .await?;
        }
        Ok(ended)
    }

    /// The board's live standing authorizations by the database's clock, newest first.
    pub async fn live_standing(&self) -> BridgeResult<Vec<Standing>> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let settings: Settings = self.configuration(connection, "settings").await?;
            let mut live: Vec<Standing> = settings
                .standing
                .into_iter()
                .filter(|s| s.live(now))
                .collect();
            live.sort_by_key(|s| std::cmp::Reverse(s.granted_at));
            Ok(live)
        })
    }

    /// Exact steps people approved at least `SUGGEST_AFTER` times in the last 30 days and
    /// never denied, that nothing approves ahead of time yet.
    pub async fn suggestions(&self) -> BridgeResult<Vec<Suggestion>> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow: Workflow = self.configuration(connection, "workflow").await?;
            let settings: Settings = self.configuration(connection, "settings").await?;
            let approved = landing(&workflow, Verdict::Approved)?;
            let denied = landing(&workflow, Verdict::Denied)?;
            let asked: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT labels,description,state FROM board_tickets
                 WHERE tenant_id=$1 AND board_id=$2 AND kind=$3 AND created_at>=$4",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(KIND)
            .bind(now - 30 * DAY_MS)
            .fetch_all(&mut *connection)
            .await?;
            let mut tally: std::collections::BTreeMap<
                (String, Option<String>, String),
                (usize, usize),
            > = Default::default();
            for (labels, description, state) in asked {
                let labels: Vec<String> = decode(&labels)?;
                let Some(kind) = label_value(&labels, "kind:") else {
                    continue;
                };
                let target = label_value(&labels, "target:");
                let Some((what, _, _)) = parts(&description) else {
                    continue;
                };
                let entry = tally.entry((kind, target, what.to_string())).or_default();
                if state == approved {
                    entry.0 += 1;
                } else if state == denied {
                    entry.1 += 1;
                }
            }
            let live: Vec<&Standing> = settings.standing.iter().filter(|s| s.live(now)).collect();
            Ok(tally
                .into_iter()
                .filter(|((kind, target, what), (yes, no))| {
                    *no == 0
                        && *yes >= SUGGEST_AFTER
                        && !live.iter().any(|s| s.covers(kind, target.as_deref(), what))
                })
                .map(|((kind, target, what), (yes, _))| Suggestion {
                    kind,
                    target,
                    what,
                    approved: yes,
                })
                .collect())
        })
    }
}

pub(super) fn checked_days(days: u32) -> BridgeResult<u32> {
    if (1..=MAX_STANDING_DAYS).contains(&days) {
        Ok(days)
    } else {
        Err(invalid(format!(
            "approve ahead of time for 1 to {MAX_STANDING_DAYS} days"
        )))
    }
}

/// A standing authorization's id, made before its transaction (G9): `st-` and 8 hex digits.
pub(super) fn standing_id() -> String {
    format!("st-{}", hex::encode(devagotchi_store::random::<4>()))
}
