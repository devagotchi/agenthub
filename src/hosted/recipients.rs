//! Recipient validation composed into the board transaction by its identity-owning host.
use super::*;
use std::{future::Future, pin::Pin, sync::Arc};

/// The host reads its own identity tables on this already scoped transaction. It must
/// not change the scope, commit separately or treat storage failure as an unknown name.
pub trait PrincipalDirectory: Send + Sync {
    /// Refuse removal while this account's explicit project credentials still refer to
    /// the name. Runs on the same scoped transaction; unknown hosts fail closed.
    fn project_retirement<'a>(
        &'a self,
        _connection: &'a mut AnyConnection,
        _tenant: &'a str,
        _account: &'a str,
        _project: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), Failed>> + Send + 'a>> {
        Box::pin(async {
            Err(fail("not_granted", "project credential directory is unavailable").into())
        })
    }

    /// Recheck the authenticated account/profile and its policy snapshot before a
    /// receipt can be returned. A changed snapshot must refuse and be authenticated anew.
    fn authorize_request<'a>(
        &'a self,
        connection: &'a mut AnyConnection,
        tenant: &'a str,
        principal: &'a str,
        actor: &'a Actor<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Failed>> + Send + 'a>>;

    fn enabled<'a>(
        &'a self,
        connection: &'a mut AnyConnection,
        tenant: &'a str,
        principal: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, Failed>> + Send + 'a>>;
}

impl HostedBoard {
    pub fn with_principals(mut self, directory: Arc<dyn PrincipalDirectory>) -> Self {
        self.principals = Some(directory);
        self
    }

    pub(super) async fn recipient(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        to: &str,
    ) -> DbResult<()> {
        // Ordinary legacy seats retain their existing contract. A reserved registered
        // identity, or any target named by a registered caller, requires live evidence.
        if !to.starts_with("@agent:") && actor.policy.is_none() {
            return if crate::valid_name(to) {
                Ok(())
            } else {
                Err(invalid("recipient must be an enabled principal or a valid seat name").into())
            };
        }
        if let Some(directory) = &self.principals
            && directory.enabled(connection, &self.tenant, to).await?
        {
            return Ok(());
        }
        Err(invalid("recipient is not an enabled principal in this environment").into())
    }
}
