//! One authenticated adapter request, composed into the domain mutation transaction.
use super::*;
use devagotchi_store::receipts::{self, Outcome, Request, Reservation};
use serde::Serialize;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub(super) struct Mutation {
    principal: String,
    key: String,
    operation: String,
    input: serde_json::Value,
    replayed: Arc<AtomicBool>,
}

pub(super) enum Receipt<T> {
    Fresh(Option<Reservation>),
    Replay(T),
}

impl<T: Serialize> Receipt<T> {
    pub async fn complete(self, connection: &mut AnyConnection, result: T) -> DbResult<T> {
        if let Self::Fresh(Some(reservation)) = self {
            reservation.complete_in(connection, &result).await?;
        }
        Ok(result)
    }
}

impl HostedBoard {
    pub(super) fn event_request_id(&self) -> Option<&str> {
        self.mutation.as_ref().map(|request| request.key.as_str())
    }

    /// A request-local copy. The host supplies the authenticated principal and complete
    /// typed input BEFORE adding transport defaults (session ids, remembered fences).
    /// Neither authority nor the principal may come from that input. Never cache this copy.
    pub fn requested<T: Serialize>(
        &self,
        principal: &str,
        key: &str,
        operation: &str,
        input: &T,
    ) -> BridgeResult<Self> {
        let input =
            serde_json::to_value(input).map_err(|_| invalid("request input cannot be encoded"))?;
        Request::new(&self.tenant, principal, key, operation, &input)?;
        let mut board = self.clone();
        board.mutation = Some(Mutation {
            principal: principal.into(),
            key: key.into(),
            operation: operation.into(),
            input,
            replayed: Arc::new(AtomicBool::new(false)),
        });
        Ok(board)
    }

    /// Only inspect after the operation succeeds. A replay is its original result,
    /// including original lease expiry; it does not renew a lease or grant new authority.
    pub fn replayed(&self) -> bool {
        self.mutation
            .as_ref()
            .is_some_and(|m| m.replayed.load(Ordering::Relaxed))
    }

    pub(crate) fn replay_flag(&self) -> Arc<AtomicBool> {
        self.mutation
            .as_ref()
            .map(|mutation| mutation.replayed.clone())
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
    }

    /// The host directory checks current account/profile status before receipt access.
    /// Callers must check current resource policy before returning either outcome. A
    /// state-dependent operation retains its original action and rechecks that action
    /// against current labels; claim-next only learns its original resource from replay.
    pub(super) async fn begin_mutation<T: DeserializeOwned>(
        &self,
        connection: &mut AnyConnection,
        actor: &Actor<'_>,
        operation: &str,
    ) -> DbResult<Receipt<T>> {
        let Some(mutation) = &self.mutation else {
            return Ok(Receipt::Fresh(None));
        };
        mutation.replayed.store(false, Ordering::Relaxed);
        if mutation.operation != operation {
            return Err(invalid("request operation does not match the mutation").into());
        }
        if let Some(directory) = &self.principals {
            directory
                .authorize_request(connection, &self.tenant, &mutation.principal, actor)
                .await?;
        } else if self.owner.is_some() {
            return Err(fail(
                "not_granted",
                "hosted request identity directory is unavailable",
            )
            .into());
        }
        let request = Request::new(
            &self.tenant,
            &mutation.principal,
            &mutation.key,
            operation,
            &serde_json::json!({"board":self.project,"actor":actor.id,"human":actor.human,"input":mutation.input}),
        )?;
        match receipts::begin_in(connection, &request).await? {
            Outcome::Fresh(reservation) => Ok(Receipt::Fresh(Some(reservation))),
            Outcome::Replay(value) => {
                let result = serde_json::from_value(value).map_err(|_| {
                    fail(
                        "receipt_damaged",
                        "stored board response has an invalid shape",
                    )
                })?;
                mutation.replayed.store(true, Ordering::Relaxed);
                Ok(Receipt::Replay(result))
            }
        }
    }
}
