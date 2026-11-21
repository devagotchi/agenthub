//! Task lifetime evidence composed into machine-access transactions by the host.
//! The board owns generation, holder, fence, lease identity and expiry checks.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskLeaseBinding {
    pub tenant: String,
    pub board: String,
    pub project: String,
    pub ticket: String,
    pub principal: String,
    pub fence: i64,
    pub lease_id: String,
    /// Database-clock milliseconds. Evidence at the time of this check, not a renewal.
    pub expires_at: i64,
    /// Current labels for the host's policy evaluation; they are not lease identity.
    pub labels: Vec<String>,
}

impl HostedBoard {
    /// Called with a board resolved for the authenticated account and canonical actor.
    /// This takes the board lock FIRST, before policy/catalog/access locks. It neither
    /// commits the transaction nor grants machine authority. The host must separately
    /// check its current machine, access policy and grant inside this transaction.
    pub async fn bind_access_task_in(
        &self,
        connection: &mut AnyConnection,
        principal: &str,
        actor: &Actor<'_>,
        ticket: &str,
        fence: i64,
    ) -> DbResult<TaskLeaseBinding> {
        if principal != actor.id || principal.is_empty() || fence < 1 {
            return Err(fail(
                "not_granted",
                "task access needs its canonical principal and current fence",
            )
            .into());
        }
        let now = self.scope(connection).await?;
        let directory = self.principals.as_ref().ok_or_else(|| {
            fail(
                "not_granted",
                "task access identity directory is unavailable",
            )
        })?;
        directory
            .authorize_request(connection, &self.tenant, principal, actor)
            .await?;
        let workflow: Workflow = self.configuration(connection, "workflow").await?;
        let task = self.ticket(connection, ticket, &workflow, now).await?;
        crate::lease::check_held(&task, principal, Some(fence))?;
        if workflow.is_done(&task.state) || workflow.lane_by_active(&task.state).is_none() {
            return Err(fail("not_granted", "task is not in an active work stage").into());
        }
        let lease_id = task
            .lease_id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| fail("board_damaged", "held task has no lease identity"))?;
        let expires_at = task
            .lease_expires_at
            .filter(|at| *at > now)
            .ok_or_else(|| fail("lease_expired", "task access lease is not live"))?;
        Ok(TaskLeaseBinding {
            tenant: self.tenant.clone(),
            board: self.project.clone(),
            project: self.name.clone(),
            ticket: task.id,
            principal: principal.into(),
            fence,
            lease_id,
            expires_at,
            labels: task.labels,
        })
    }

    /// Recheck the original generation and lease after heartbeat, release or reclaim.
    /// The binding is stored by the host; callers may not supply it as authentication.
    /// A live heartbeat may extend expiry, but cannot change any identity in the binding.
    pub async fn check_access_task_in(
        &self,
        connection: &mut AnyConnection,
        principal: &str,
        actor: &Actor<'_>,
        binding: &TaskLeaseBinding,
    ) -> DbResult<TaskLeaseBinding> {
        if binding.tenant != self.tenant
            || binding.project != self.name
            || binding.board != self.project
            || binding.principal != principal
        {
            return Err(fail(
                "not_granted",
                "task access belongs to another principal or board generation",
            )
            .into());
        }
        let current = self
            .bind_access_task_in(connection, principal, actor, &binding.ticket, binding.fence)
            .await?;
        if current.lease_id != binding.lease_id {
            return Err(fail("stale_fence", "task access lease identity changed").into());
        }
        Ok(current)
    }
}
