//! Target inboxes and integrator channels (docs/16-external-dialogs.md, P418).
//!
//! A target whose `yaacd` offers `channels` reports its integrator channels
//! (`channel/registered`, `channel/closed`) and the dialogs raised without a thread (its
//! inbox: `inbox/list`, `inbox/updated`). The hub keeps both in memory — never in the cache
//! file, so nothing an integrator asked is written to the host — and tells every window the
//! whole inbox of a target when it changes (`hub/inbox/changed`). An inbox request is reminded
//! of like any other request, under "<integrator> on <target>". Answers go to the target over
//! its one connection as an ordinary routed `inbox/respond` (invariant 1).

use super::*;
use attach::{HubChannel, InboxListResult, TargetInbox};
use yaac_protocol::{ChannelClosedParams, ChannelRegisteredParams, InboxUpdatedParams, RequestBody, RequestItem, RequestResponse};

/// Whether an inbox request is still the user's: open, or answered with an unknown outcome.
fn kept(r: &RequestItem) -> bool {
    matches!(r.state, RequestState::Pending | RequestState::Submitted | RequestState::Unknown)
}

/// Withholds the values of secret inputs from a dialog's recorded answer: they never reach
/// the host's memory beyond the answer the window sends.
pub(crate) fn redact_dialog(r: &mut RequestItem) {
    let RequestBody::Dialog(d) = &r.body else { return };
    if let Some(res) = r.resolution.as_mut()
        && let Some(RequestResponse::Dialog(answer)) = res.answer.as_mut()
    {
        yaac_protocol::dialog_tree::redact(&d.layout, &mut answer.values);
    }
}

impl Core {
    /// After a connection is ready: read the target's inbox, when it has one.
    pub(super) fn inbox_on_ready(&mut self, id: &TargetId) {
        let channels = self
            .targets
            .get(id)
            .and_then(|t| t.conn.as_ref())
            .is_some_and(|c| c.session.features().is_some_and(|f| f.allows(&yaac_protocol::Capability::Channels)));
        if channels {
            self.send::<methods::InboxList>(id, &yaac_protocol::Empty {}, Pending::InboxList);
        } else if self.inboxes.remove(id).is_some() {
            // A daemon without channels has no inbox (an older one took over).
            self.inbox_changed(id);
        }
    }

    pub(super) fn inbox_listed(&mut self, id: &TargetId, result: yaac_protocol::InboxListResult) {
        let inbox = self.inboxes.entry(id.clone()).or_insert_with(|| TargetInbox { target: id.clone(), ..TargetInbox::default() });
        inbox.channels = result
            .channels
            .into_iter()
            .map(|c| HubChannel { channel_id: c.channel_id, name: c.name, description: c.description, connected: true })
            .collect();
        inbox.requests = result.requests.into_iter().filter(kept).collect();
        inbox.requests.iter_mut().for_each(redact_dialog);
        self.inbox_changed(id);
    }

    /// `inbox/updated`, `channel/registered`, `channel/closed`. Returns whether it was one.
    pub(super) fn inbox_notification(&mut self, id: &TargetId, method: &str, params: &Value) -> bool {
        let inbox = self.inboxes.entry(id.clone()).or_insert_with(|| TargetInbox { target: id.clone(), ..TargetInbox::default() });
        match method {
            names::INBOX_UPDATED => {
                let Ok(p) = serde_json::from_value::<InboxUpdatedParams>(params.clone()) else { return true };
                let mut request = p.request;
                redact_dialog(&mut request);
                let at = inbox.requests.iter().position(|r| r.request_id == request.request_id);
                match (at, kept(&request)) {
                    (Some(i), true) => inbox.requests[i] = request,
                    (None, true) => inbox.requests.push(request),
                    (Some(i), false) => {
                        inbox.requests.remove(i);
                    }
                    (None, false) => {}
                }
            }
            names::CHANNEL_REGISTERED => {
                let Ok(p) = serde_json::from_value::<ChannelRegisteredParams>(params.clone()) else { return true };
                inbox.channels.retain(|c| c.channel_id != p.channel_id);
                inbox.channels.push(HubChannel { channel_id: p.channel_id, name: p.name, description: p.description, connected: true });
            }
            names::CHANNEL_CLOSED => {
                let Ok(p) = serde_json::from_value::<ChannelClosedParams>(params.clone()) else { return true };
                for c in inbox.channels.iter_mut().filter(|c| c.channel_id == p.channel_id) {
                    c.connected = false;
                }
            }
            _ => return false,
        }
        self.inbox_changed(id);
        true
    }

    /// A removed target takes its inbox with it.
    pub(super) fn inbox_target_removed(&mut self, id: &TargetId) {
        if self.inboxes.remove(id).is_some() {
            self.inbox_changed(id);
        }
    }

    /// `hub/inbox/list`.
    pub(super) fn inbox_list(&self) -> Value {
        to_value(&InboxListResult { targets: self.inboxes.values().cloned().collect() })
    }

    /// Tells every window the target's whole inbox, and keeps the reminder in step.
    fn inbox_changed(&mut self, id: &TargetId) {
        let inbox = self.inboxes.get(id).cloned().unwrap_or_else(|| TargetInbox { target: id.clone(), ..TargetInbox::default() });
        let target_name = self.targets.get(id).map_or_else(|| id.as_str().to_owned(), |t| t.def.name.to_string());
        let reminded: Vec<(RequestItem, String)> = inbox
            .requests
            .iter()
            .map(|r| {
                let source = match &r.body {
                    RequestBody::Dialog(d) => d.source.name.clone(),
                    _ => "A program".to_owned(),
                };
                (r.clone(), format!("{source} on {target_name}"))
            })
            .collect();
        self.notifications.snapshot_inbox(id, &reminded);
        self.notify_all(attach::names::INBOX_CHANGED, to_value(&inbox));
    }
}
