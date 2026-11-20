//! A board's GitHub issues: the issue a ticket was projected from, the shared context agents
//! read for one issue, and a checkpoint written into it.

use devagotchi_contracts::BridgeResult;

use crate::issue_lifecycle::{ContextReceipt, ContextWrite, IssueContext, SourceIssue};
use crate::{Actor, Board};

impl Board {
    /// The GitHub issue `id` was projected from, if it was.
    pub fn issue_source(&self, id: &str) -> BridgeResult<Option<SourceIssue>> {
        self.hosted_call(self.hosted.issue_source(id))
    }

    /// One page of issue `number`'s shared context in `repository`, after note `after`.
    pub fn issue_context(
        &self,
        repository: u64,
        number: u64,
        after: i64,
        limit: usize,
    ) -> BridgeResult<IssueContext> {
        self.hosted_call(self.hosted.issue_context(repository, number, after, limit))
    }

    /// A checkpoint into an issue's shared context, once per `request_id`.
    pub fn write_issue_context(
        &self,
        actor: &Actor<'_>,
        repository: u64,
        payload: &ContextWrite,
    ) -> BridgeResult<ContextReceipt> {
        self.hosted_call(self.hosted.write_issue_context(actor, repository, payload))
    }
}
