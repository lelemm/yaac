//! SSH setup through the hub (`hub/ssh/*`, the hub's `protocol::names::SSH_*`): the window
//! sends each step as a request and hands the answers to its setup backend
//! ([`Client::ssh_request`], [`Client::take_ssh_replies`]); the record of what YAAC did on
//! each server comes from the hub (`hub/ssh/records`, refreshed on `hub/ssh/changed`) and is
//! shown on every SSH target of that server.

use serde::Deserialize;
use serde_json::{Value, json};
use yaac_protocol::attach::TargetTransport;
use yaac_protocol::rpc::{ErrorObject, RpcId};

use super::{Call, Client, hub_names};
use crate::ssh_setup::{InstalledKey, SetupRecord};

/// One `hub/ssh/records` entry, as the window reads it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SshWire {
    host: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    user: Option<String>,
    record: SshWireRecord,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SshWireRecord {
    kind: String,
    #[serde(default)]
    line: Option<String>,
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    at_ms: i64,
}

/// `(host, port, user)` with the defaults filled in, as the hub compares servers.
fn server(host: &str, port: Option<u16>, user: Option<&str>) -> (String, u16, String) {
    (host.trim().to_owned(), port.unwrap_or(22), user.unwrap_or("").trim().to_owned())
}

impl Client {
    /// Send one `hub/ssh/*` step; its answer comes back through [`Client::take_ssh_replies`]
    /// under the returned id.
    pub fn ssh_request(&mut self, method: &str, params: Value) -> RpcId {
        let id = RpcId::Number(self.next_id);
        self.request(method, params, Call::Ssh(id))
    }

    /// Answers to [`Client::ssh_request`] that arrived since the last call.
    pub fn take_ssh_replies(&mut self) -> Vec<(RpcId, Result<Value, ErrorObject>)> {
        std::mem::take(&mut self.ssh_replies)
    }

    /// The user removed YAAC's line by hand: the hub deletes YAAC's key for that server.
    pub fn ssh_removed_by_hand(&mut self, target: &yaac_protocol::TargetId) {
        let Some(TargetTransport::Ssh(s)) = self.state.target(target).map(|t| t.entry.transport.clone()) else { return };
        let params = json!({ "host": s.host, "port": s.port, "user": s.user });
        self.request(hub_names::SSH_REMOVED_BY_HAND, params, Call::Ignore);
    }

    pub(super) fn load_ssh_records(&mut self) {
        self.request(hub_names::SSH_RECORDS, json!({}), Call::SshRecords);
    }

    pub(super) fn on_ssh_records(&mut self, result: &Value) {
        let records = result.get("records").cloned().unwrap_or(Value::Null);
        if let Ok(list) = serde_json::from_value::<Vec<SshWire>>(records) {
            self.ssh_wire = Some(list);
            self.map_ssh_records();
        }
    }

    /// Show the hub's records on the SSH targets (once the hub has answered; until then, and
    /// with the demo hub, the records stay window-local).
    pub(super) fn map_ssh_records(&mut self) {
        let Some(wire) = &self.ssh_wire else { return };
        let zone = self.ssh_zone;
        let date = |ms: i64| crate::time::date_in(ms.div_euclid(1000), zone);
        let mut mapped = Vec::new();
        for t in &self.state.targets {
            let TargetTransport::Ssh(s) = &t.entry.transport else { continue };
            let me = server(&s.host, s.port, s.user.as_deref());
            let found = wire.iter().find(|w| server(&w.host, w.port, w.user.as_deref()) == me);
            let record = found.map_or(SetupRecord::NotSetUp, |w| {
                let r = &w.record;
                let key = || InstalledKey { line: r.line.clone().unwrap_or_default(), comment: r.comment.clone().unwrap_or_default() };
                match r.kind.as_str() {
                    // A line still being proved can be revoked too.
                    "installedByYaac" | "installing" => SetupRecord::InstalledByYaac { key: key(), date: date(r.at_ms) },
                    "alreadyInPlace" => SetupRecord::AlreadyInPlace { date: date(r.at_ms) },
                    "revoked" => SetupRecord::Revoked { date: date(r.at_ms) },
                    "removedByHand" => SetupRecord::RemovedByHand { date: date(r.at_ms) },
                    _ => SetupRecord::NotSetUp,
                }
            });
            mapped.push((t.entry.id.clone(), record));
        }
        for (id, record) in mapped {
            if record == SetupRecord::NotSetUp {
                self.state.ssh_records.remove(&id);
            } else {
                self.state.ssh_records.insert(id, record);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use yaac_protocol::ClientId;
    use yaac_protocol::rpc::Message;

    use super::*;
    use crate::link::HubLink;

    #[derive(Default)]
    struct Wire {
        sent: Vec<Message>,
        inbox: VecDeque<Message>,
    }

    struct Mock(Rc<RefCell<Wire>>);

    impl HubLink for Mock {
        fn send(&mut self, message: Message) {
            self.0.borrow_mut().sent.push(message);
        }
        fn poll(&mut self) -> Option<Message> {
            self.0.borrow_mut().inbox.pop_front()
        }
    }

    fn answer(wire: &Rc<RefCell<Wire>>, method: &str, result: Value) {
        let id = wire
            .borrow()
            .sent
            .iter()
            .rev()
            .find_map(|m| match m {
                Message::Request { id, method: m, .. } if m == method => Some(id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{method} was not sent"));
        wire.borrow_mut().inbox.push_back(Message::Response { id, result });
    }

    #[test]
    fn the_hubs_records_are_shown_on_every_target_of_that_server_and_steps_get_their_answers() {
        let wire = Rc::new(RefCell::new(Wire::default()));
        let mut c = Client::new(Box::new(Mock(wire.clone())), "b", ClientId::new("w"));
        let target = |id: &str, port: Option<u16>| {
            json!({ "id": id, "name": id, "transport": { "kind": "ssh", "host": "devbox", "port": port }, "enabled": true, "state": { "status": "offline" } })
        };
        answer(&wire, "hub/hello", json!({ "build": "b", "targets": [target("t1", None), target("t2", Some(22)), target("t3", Some(2222))] }));
        c.pump();
        let at_ms = 1_795_197_600_000_i64;
        let records = json!({ "records": [
            { "host": "devbox", "port": 22, "record": { "kind": "installedByYaac", "line": "ssh-ed25519 AAAA yaac:h:1", "comment": "yaac:h:1", "atMs": at_ms } }
        ] });
        answer(&wire, hub_names::SSH_RECORDS, records);
        c.pump();
        let t1 = yaac_protocol::TargetId::new("t1");
        assert!(matches!(c.state.ssh_records.get(&t1), Some(SetupRecord::InstalledByYaac { key, date }) if key.comment == "yaac:h:1" && date == "2026-11-20"));
        assert!(c.state.ssh_records.contains_key(&yaac_protocol::TargetId::new("t2")), "port 22 written out is the same server");
        assert!(!c.state.ssh_records.contains_key(&yaac_protocol::TargetId::new("t3")), "another port is another server");

        let id = c.ssh_request(hub_names::SSH_VERIFY, json!({ "host": "devbox" }));
        answer(&wire, hub_names::SSH_VERIFY, json!({}));
        c.pump();
        assert_eq!(c.take_ssh_replies(), vec![(id, Ok(json!({})))]);
        assert!(c.take_ssh_replies().is_empty());
    }
}
