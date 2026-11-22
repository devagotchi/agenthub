//! The dependency graph: what blocks what, and where a ticket came from.

use super::*;
use crate::chain::actions;
use crate::{Dep, DepKind};

impl HostedBoard {
    pub(super) async fn deps(
        &self,
        connection: &mut AnyConnection,
        id: &str,
        reverse: bool,
    ) -> DbResult<Vec<Dep>> {
        let rows = sqlx::query(
            "SELECT ticket,depends_on,kind FROM board_deps WHERE tenant_id=$1 AND board_id=$2
             AND (($4=0 AND ticket=$3) OR ($4=1 AND depends_on=$3))
             ORDER BY kind COLLATE \"C\",ticket COLLATE \"C\",depends_on COLLATE \"C\"",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(id)
        .bind(i64::from(reverse))
        .fetch_all(&mut *connection)
        .await?;
        let mut deps = Vec::with_capacity(rows.len());
        for row in rows {
            let kind: String = row.try_get("kind")?;
            deps.push(Dep {
                ticket: row.try_get("ticket")?,
                depends_on: row.try_get("depends_on")?,
                kind: DepKind::parse(&kind)
                    .ok_or_else(|| fail("board_damaged", "invalid dependency kind"))?,
            });
        }
        Ok(deps)
    }

    pub async fn add_dep(
        &self,
        actor: &Actor<'_>,
        id: &str,
        depends_on: &str,
        kind: DepKind,
    ) -> BridgeResult<Vec<Dep>> {
        if id == depends_on {
            return Err(invalid("a ticket cannot depend on itself"));
        }
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            self.authorize(connection, actor, "task.edit", &ticket.labels)
                .await?;
            let receipt = self.begin_mutation(connection, actor, "add_dep").await?;
            if let Receipt::Replay(result) = receipt {
                return Ok(result);
            }
            self.ticket(connection, depends_on, &workflow, now).await?;
            if kind == DepKind::Blocks {
                let count:i64=sqlx::query_scalar(
                    "WITH RECURSIVE walk(id) AS (
                       SELECT $3::text UNION SELECT d.depends_on FROM board_deps d JOIN walk ON d.ticket=walk.id
                       WHERE d.tenant_id=$1 AND d.board_id=$2 AND d.kind='blocks'
                     ) SELECT count(*) FROM walk WHERE id=$4",
                ).bind(&self.tenant).bind(&self.project).bind(depends_on).bind(id)
                    .fetch_one(&mut *connection).await?;
                if count > 0 {
                    return Err(fail(
                        "dependency_cycle",
                        format!("{depends_on} already depends on {id}"),
                    )
                    .into());
                }
            }
            let added = sqlx::query(
                "INSERT INTO board_deps (tenant_id,board_id,ticket,depends_on,kind,created_by,created_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING",
            ).bind(&self.tenant).bind(&self.project).bind(id).bind(depends_on).bind(kind.as_str())
                .bind(actor.id).bind(now).execute(&mut *connection).await?.rows_affected();
            // Already there: the graph as it is, nothing written, nobody woken (rule 6).
            if added == 0 {
                let result = self.deps(connection, id, false).await?;
                return receipt.complete(connection, result).await;
            }
            self.event(
                connection,
                actor.id,
                now,
                actions::DEP_ADDED,
                Some(id),
                None,
                serde_json::json!({"depends_on":depends_on,"kind":kind.as_str()}),
            )
            .await?;
            let result = self.deps(connection, id, false).await?;
            receipt.complete(connection, result).await
        })
    }

    pub async fn remove_dep(
        &self,
        actor: &Actor<'_>,
        id: &str,
        depends_on: &str,
        kind: DepKind,
    ) -> BridgeResult<Vec<Dep>> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let workflow = self.configuration(connection, "workflow").await?;
            let ticket = self.ticket(connection, id, &workflow, now).await?;
            self.authorize(connection, actor, "task.edit", &ticket.labels)
                .await?;
            let receipt = self.begin_mutation(connection, actor, "remove_dep").await?;
            if let Receipt::Replay(result) = receipt {
                return Ok(result);
            }
            let removed=sqlx::query(
                "DELETE FROM board_deps WHERE tenant_id=$1 AND board_id=$2 AND ticket=$3 AND depends_on=$4 AND kind=$5",
            ).bind(&self.tenant).bind(&self.project).bind(id).bind(depends_on).bind(kind.as_str())
                .execute(&mut *connection).await?.rows_affected();
            if removed == 0 {
                return Err(fail("not_found", "no such dependency").into());
            }
            self.event(
                connection,
                actor.id,
                now,
                actions::DEP_REMOVED,
                Some(id),
                None,
                serde_json::json!({"depends_on":depends_on,"kind":kind.as_str()}),
            )
            .await?;
            let result = self.deps(connection, id, false).await?;
            receipt.complete(connection, result).await
        })
    }
}
