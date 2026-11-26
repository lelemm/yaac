//! Route daemon browser calls to a window on the attach connection.

use std::collections::{HashMap, VecDeque};

use yaac_protocol::BrowserCallParams;

use super::{
    ClientKey, Core, Duration, ErrorCode, ErrorObject, Hello, Instant, Message, RpcId, TargetId,
    Value, attach, err, names, to_value,
};

const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct Call {
    source: TargetId,
    generation: u64,
    peer_id: RpcId,
    params: Value,
    peer_method: &'static str,
    attach_method: &'static str,
    candidates: VecDeque<ClientKey>,
    deadline: Instant,
}

#[derive(Debug)]
struct Pending {
    window: ClientKey,
    call: Call,
}

#[derive(Debug, Default)]
pub(super) struct BrowserCalls {
    serial: u64,
    pending: HashMap<RpcId, Pending>,
}

impl Core {
    pub(super) fn route_browser(
        &mut self,
        source: &TargetId,
        generation: u64,
        peer_id: RpcId,
        params: BrowserCallParams,
    ) {
        let thread_id = params.thread_id.clone();
        let payload = to_value(&attach::BrowserCallParams {
            target: source.clone(),
            call: params,
        });
        self.route_window(
            source,
            generation,
            peer_id,
            thread_id,
            payload,
            names::BROWSER_CALL,
            attach::names::BROWSER_CALL,
        );
    }

    pub(super) fn route_theme(
        &mut self,
        source: &TargetId,
        generation: u64,
        peer_id: RpcId,
        params: yaac_protocol::ThemeCallParams,
    ) {
        self.route_window(
            source,
            generation,
            peer_id,
            params.thread_id.clone(),
            to_value(&params),
            names::THEME_CALL,
            attach::names::THEME_CALL,
        );
    }

    fn route_window(
        &mut self,
        source: &TargetId,
        generation: u64,
        peer_id: RpcId,
        thread_id: yaac_protocol::ThreadId,
        params: Value,
        peer_method: &'static str,
        attach_method: &'static str,
    ) {
        let thread = (source.clone(), thread_id);
        let mut windows: Vec<_> = self
            .clients
            .iter()
            .filter(|(_, client)| client.hello == Hello::Attached)
            .map(|(&id, client)| {
                (
                    !client.showing.contains(&thread),
                    !client.subs.contains(&thread),
                    id,
                )
            })
            .collect();
        windows.sort_unstable();
        self.offer_browser(Call {
            source: source.clone(),
            generation,
            peer_id,
            params,
            peer_method,
            attach_method,
            candidates: windows.into_iter().map(|(_, _, window)| window).collect(),
            deadline: Instant::now() + ANSWER_TIMEOUT,
        });
    }

    fn offer_browser(&mut self, mut call: Call) {
        while let Some(window) = call.candidates.pop_front() {
            let Some(client) = self
                .clients
                .get(&window)
                .filter(|c| c.hello == Hello::Attached)
            else {
                continue;
            };
            self.browser.serial += 1;
            let id = RpcId::String(format!("browser-{}", self.browser.serial));
            let msg = Message::request(id.clone(), call.attach_method, call.params.clone());
            if client.tx.send(msg).is_ok() {
                self.browser.pending.insert(id, Pending { window, call });
                return;
            }
        }
        let theme = call.peer_method == names::THEME_CALL;
        self.finish_browser(
            call,
            Err(err(
                ErrorCode::NoWindow,
                if theme {
                    "No YAAC window is open for this theme request"
                } else {
                    "No YAAC window is open; the browser needs one"
                },
            )),
        );
    }

    pub(super) fn browser_answer(
        &mut self,
        window: ClientKey,
        id: &RpcId,
        outcome: Result<Value, ErrorObject>,
    ) {
        // A response on another attach socket cannot claim this request.
        if !self
            .browser
            .pending
            .get(id)
            .is_some_and(|p| p.window == window)
        {
            return;
        }
        let Some(Pending { call, .. }) = self.browser.pending.remove(id) else {
            return;
        };
        if outcome
            .as_ref()
            .is_err_and(|e| e.code == ErrorCode::MethodNotFound)
        {
            // The window explicitly declined the attach method, so offer it to the next one.
            self.offer_browser(call);
        } else {
            self.finish_browser(call, outcome);
        }
    }

    pub(super) fn browser_window_closed(&mut self, window: ClientKey) {
        let calls: Vec<_> = self
            .browser
            .pending
            .extract_if(|_, pending| pending.window == window)
            .map(|(_, pending)| pending.call)
            .collect();
        for call in calls {
            let error = err(
                ErrorCode::InternalError,
                "the window handling the browser request closed",
            );
            self.finish_browser(call, Err(error));
        }
    }

    pub(super) fn browser_target_closed(&mut self, source: &TargetId) {
        self.browser
            .pending
            .retain(|_, pending| &pending.call.source != source);
    }

    pub(super) fn expire_browser_calls(&mut self, now: Instant) {
        let calls: Vec<_> = self
            .browser
            .pending
            .extract_if(|_, pending| pending.call.deadline <= now)
            .map(|(_, pending)| pending.call)
            .collect();
        for call in calls {
            let error = err(
                ErrorCode::InternalError,
                "the window did not answer the browser request in time",
            );
            self.finish_browser(call, Err(error));
        }
    }

    fn finish_browser(&mut self, call: Call, outcome: Result<Value, ErrorObject>) {
        self.respond_peer(
            &call.source,
            call.generation,
            call.peer_id,
            call.peer_method,
            outcome,
        );
    }
}
