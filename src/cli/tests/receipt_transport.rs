//! Older and malformed peers cannot silently downgrade a prepared mutation.
use crate::{
    Actor, DepKind, NewTicket,
    cli::{Remote, Session},
    face::Face,
};
use devagotchi_contracts::BridgeResult;
use serde_json::{Value, json};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

#[derive(Default)]
struct Exchange {
    answers: VecDeque<Value>,
    calls: Vec<(String, Value)>,
}

struct Peer(Rc<RefCell<Exchange>>);

impl Session for Peer {
    fn hub(&self) -> &str {
        "https://hub.test"
    }
    fn call_tool(&mut self, name: &str, arguments: Value) -> BridgeResult<Value> {
        let mut exchange = self.0.borrow_mut();
        exchange.calls.push((name.into(), arguments));
        Ok(exchange.answers.pop_front().expect("unexpected tool call"))
    }
}

#[test]
fn older_hub_must_advertise_receipts_before_cli_write() {
    let exchange = Rc::new(RefCell::new(Exchange {
        answers: [json!({"subject":"account"})].into(),
        ..Default::default()
    }));
    let remote = Remote::new(Box::new(Peer(exchange.clone())), "demo", "D");
    let actor = Actor::agent("worker");
    assert_eq!(
        remote.request_identity(&actor).unwrap_err().code,
        "not_supported"
    );
    assert_eq!(exchange.borrow().calls.len(), 1);
    assert_eq!(exchange.borrow().calls[0].0, "whoami");
}

#[test]
fn remote_requires_matching_receipt_and_refuses_a_different_mutation() {
    let actor = Actor::agent("worker");
    for answer in [
        json!({"deps":[]}),
        json!({"deps":[],"request_id":"different","replayed":true}),
        json!({"deps":[],"request_id":"original","replayed":"true"}),
        json!({"deps":[],"request_id":"original","replayed":true}),
    ] {
        let confirmed = answer["request_id"] == "original" && answer["replayed"] == true;
        let exchange = Rc::new(RefCell::new(Exchange {
            answers: [answer].into(),
            ..Default::default()
        }));
        let mut remote = Remote::new(Box::new(Peer(exchange.clone())), "demo", "D");
        remote
            .set_request("account", "original", "add_dep", &json!({}))
            .unwrap();
        assert_eq!(
            remote
                .create(&actor, NewTicket::default())
                .unwrap_err()
                .code,
            "usage"
        );
        assert!(
            exchange.borrow().calls.is_empty(),
            "no unkeyed fallback write"
        );
        let result = remote.add_dep(&actor, "D-1", "D-2", DepKind::Blocks);
        if confirmed {
            assert!(result.unwrap().is_empty());
            assert!(remote.replayed());
        } else {
            assert_eq!(result.unwrap_err().code, "hub_error");
            assert!(!remote.replayed());
        }
        let exchange = exchange.borrow();
        assert_eq!(exchange.calls[0].0, "board_dep");
        assert_eq!(exchange.calls[0].1["request_id"], "original");
    }
}
