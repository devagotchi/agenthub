//! What a new board shows first: a "Getting started" ticket in the workflow's attention
//! state. The first AI session primes the board and finds the steps under `attention`;
//! the person finds them on the board, instead of an empty one. It is made once, only on
//! a board with no tickets at all, and recorded with a `board.welcomed` event. It sits
//! outside the agents' queue, so nobody claims it by accident. Its title and steps are the
//! site's (`content/messages.json` `welcome`, `Site::welcome`).

use devagotchi_contracts::BridgeResult;

use super::{Actor, Board};

pub const LABEL: &str = "getting-started";

impl Board {
    /// Put the getting-started ticket, `title` and its `steps`, on a board that has never
    /// had a ticket. Returns its id, or `None` when the board already has tickets.
    pub fn welcome_if_new(
        &self,
        actor: &Actor<'_>,
        title: &str,
        steps: &str,
    ) -> BridgeResult<Option<String>> {
        self.hosted_call(self.hosted.welcome_if_new(actor, title, steps))
    }
}
