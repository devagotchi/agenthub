//! Evaluate profile policy against the resource read by the mutation transaction.
use super::*;
use crate::policy::{Resource, Subject, catalog};

impl HostedBoard {
    /// The host has already checked this person's configuration authority for the board.
    /// Project administrators get the policy's admin role only within that authorized
    /// resource. Registered agents still cannot acquire human configuration authority.
    pub(super) async fn authorize_configuration(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        action: &str,
    ) -> DbResult<()> {
        if !actor.human {
            return if actor.policy.is_some() {
                Err(fail("humans_only", "only a person manages board configuration").into())
            } else {
                Ok(()) // Unbound callers retain the host's existing configuration contract.
            };
        }
        catalog::initialize_in(connection, &self.tenant).await?;
        let mut person = actor.clone();
        person
            .policy
            .get_or_insert_with(|| crate::policy::Identity {
                kind: "human".into(),
                roles: vec!["admin".into()],
            });
        self.authorize(connection, &person, action, &[]).await
    }

    pub(super) async fn authorize(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        action: &str,
        labels: &[String],
    ) -> DbResult<()> {
        let Some(identity) = &actor.policy else {
            return Ok(());
        };
        let subject = Subject {
            id: actor.id,
            kind: &identity.kind,
            human: actor.human,
            roles: &identity.roles,
        };
        catalog::authorize_in(
            connection,
            &self.tenant,
            &subject,
            action,
            &Resource {
                project: &self.name,
                labels,
            },
        )
        .await
    }
}
