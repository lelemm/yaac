//! A sans-I/O client session: the host side of one connection to a `yaacd`.
//!
//! It owns the frame decoder, the outbound [`Scheduler`], request ids, the `initialize`
//! handshake, and the selected [`Dialect`]. Every message after the handshake is encoded and
//! decoded through the dialect; the frozen-core calls (`initialize`, `daemon/status`,
//! `daemon/handover`) never are, so they work against any daemon, even after a failed
//! negotiation.
//!
//! ```text
//! let mut s = ClientSession::new(client_info);        // queues `initialize`
//! loop {
//!     while let Some(frame) = s.poll_transmit() { write(frame); s.on_written(len, elapsed) }
//!     for event in s.receive(&read_bytes)? { … }     // Ready, Response, Notification, …
//! }
//! ```

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use crate::dialect::{
    self, Dialect, DialectError, FeatureSet, Inbound, Negotiated, OlderSide, PeerRequest, ProtocolRange,
};
use crate::frame::{Frame, FrameDecoder, FrameError};
use crate::methods::{self, Method, Notification, names};
use crate::rpc::{ErrorCode, ErrorObject, Message, MessageError, RpcId};
use crate::schedule::{Class, FlowError, Scheduler};
use crate::types::{Capability, ClientInfo, DaemonInfo, InitializeParams, InitializeResult, StreamId, TargetFacts};

/// Facts available once the handshake succeeded. Contains no version number.
#[derive(Clone, Debug)]
pub struct ReadyInfo {
    /// The daemon's build and state format.
    pub daemon: DaemonInfo,
    /// Target facts.
    pub target: TargetFacts,
    /// Whether the daemon is older than this build (show "will be updated when idle").
    pub daemon_is_older: bool,
    /// Whether the daemon's `schemaHash` matches this build's idea of the agreed version.
    /// A mismatch is a build error worth reporting in diagnostics; it does not fail the
    /// connection.
    pub schema_hash_matches: bool,
}

/// Something that happened on the connection.
// Events are short-lived and moved once; boxing the large variants would only make every
// caller's `match` clumsier.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// The handshake succeeded; the session is ready for every method.
    Ready(ReadyInfo),
    /// The handshake failed. Only `daemon/status` and `daemon/handover` may be sent now.
    NegotiationFailed {
        /// The error, with a message to show as-is.
        error: ErrorObject,
        /// Which side must be updated, when known.
        older: Option<OlderSide>,
    },
    /// A response to one of our requests. Decode it with [`ClientSession::decode`] (or the
    /// flow that sent it).
    Response {
        /// The request's id.
        id: RpcId,
        /// The request's method.
        method: &'static str,
        /// The raw wire result, or the error.
        outcome: Result<Value, ErrorObject>,
    },
    /// A notification, decoded by the dialect. `stream/credit` has already been applied.
    Notification(Inbound),
    /// A request the daemon originated (`mesh/relay`, `mesh/query`). Answer it with
    /// [`ClientSession::respond`].
    PeerRequest {
        /// Its id, in the daemon's id space.
        id: RpcId,
        /// Its method.
        method: String,
        /// The decoded request.
        request: PeerRequest,
    },
    /// Bytes on a data stream the daemon is sending.
    Data {
        /// The stream.
        stream: StreamId,
        /// The bytes.
        bytes: Vec<u8>,
    },
    /// The daemon sent an error with no request id (it could not read one of ours).
    OrphanError(ErrorObject),
}

/// A fatal or local session error.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Framing failed; close the connection.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// A frame did not hold a valid message; close the connection.
    #[error(transparent)]
    Message(#[from] MessageError),
    /// A dialect step failed.
    #[error(transparent)]
    Dialect(#[from] DialectError),
    /// A typed decode failed.
    #[error("could not decode the result of {method}: {source}")]
    Result {
        /// Method.
        method: &'static str,
        /// Cause.
        #[source]
        source: serde_json::Error,
    },
    /// Flow control.
    #[error(transparent)]
    Flow(#[from] FlowError),
    /// The method may not be sent yet (before `initialize` succeeded), or no longer (after a
    /// failed negotiation, only the frozen daemon-management calls are allowed).
    #[error("{0} cannot be sent before the protocol handshake has succeeded")]
    NotReady(&'static str),
    /// The daemon answered a request we never sent.
    #[error("response to unknown request id {0}")]
    UnknownResponse(RpcId),
    /// The daemon agreed a version this build has no dialect for (a daemon bug).
    #[error("no dialect for the version the daemon agreed")]
    NoDialect,
}

#[derive(Debug)]
enum Phase {
    /// `initialize` sent; waiting for its answer.
    Handshake { id: RpcId },
    /// Ready: every message goes through `dialect`.
    Ready { dialect: &'static Dialect, negotiated: Negotiated, features: FeatureSet },
    /// No common version: only the frozen calls.
    Failed,
}

/// The host side of one connection. See the module docs.
#[derive(Debug)]
pub struct ClientSession {
    local: ProtocolRange,
    phase: Phase,
    next_id: i64,
    pending: HashMap<RpcId, &'static str>,
    out: Scheduler,
    decoder: FrameDecoder,
}

impl ClientSession {
    /// A session speaking this build's supported range. Queues `initialize` immediately: it is
    /// the first message on the wire.
    pub fn new(client: ClientInfo) -> Self {
        Self::with_range(dialect::SUPPORTED, client)
    }

    /// A session speaking `local` (tests and fixtures).
    pub fn with_range(local: ProtocolRange, client: ClientInfo) -> Self {
        let mut session = Self {
            local,
            phase: Phase::Failed,
            next_id: 1,
            pending: HashMap::new(),
            out: Scheduler::new(),
            decoder: FrameDecoder::new(),
        };
        let params = InitializeParams { protocol: local, client };
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        let id = session.queue_request(names::INITIALIZE, params, Class::Control);
        session.phase = Phase::Handshake { id };
        session
    }

    /// Whether the handshake has succeeded.
    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Ready { .. })
    }

    /// The selected dialect, once ready.
    pub fn dialect(&self) -> Option<&'static Dialect> {
        match self.phase {
            Phase::Ready { dialect, .. } => Some(dialect),
            _ => None,
        }
    }

    /// The negotiated outcome, once ready (opaque: it reveals no number).
    pub fn negotiated(&self) -> Option<&Negotiated> {
        match &self.phase {
            Phase::Ready { negotiated, .. } => Some(negotiated),
            _ => None,
        }
    }

    /// The wire features of the agreed version, once ready.
    pub fn features(&self) -> Option<FeatureSet> {
        match self.phase {
            Phase::Ready { features, .. } => Some(features),
            _ => None,
        }
    }

    /// Removes capabilities the agreed version cannot carry. Apply to every capability set
    /// before the UI sees it. Before the handshake it removes nothing.
    pub fn fold_capabilities(&self, capabilities: &mut Vec<Capability>) {
        if let Some(features) = self.features() {
            features.fold(capabilities);
        }
    }

    /// Sends a request. After the handshake, through the dialect; the frozen calls are always
    /// allowed and never translated.
    pub fn request<M: Method>(&mut self, params: &M::Params) -> Result<RpcId, SessionError> {
        let params = serde_json::to_value(params).map_err(|source| SessionError::Result { method: M::NAME, source })?;
        self.request_raw(M::NAME, params, M::INFO.request_class)
    }

    /// Sends a request whose params are already current-shape JSON (used by the hub's routing
    /// envelope and by flows).
    pub fn request_value(&mut self, method: &'static str, params: Value) -> Result<RpcId, SessionError> {
        let class = methods::request_info(method).map_or(Class::Control, |info| info.request_class);
        self.request_raw(method, params, class)
    }

    /// Sends a request whose params are already **wire** JSON produced by a dialect step.
    pub(crate) fn request_wire(&mut self, method: &'static str, wire: Value) -> Result<RpcId, SessionError> {
        self.ready_dialect(method)?;
        Ok(self.queue_request(method, wire, Class::Control))
    }

    fn request_raw(&mut self, method: &'static str, params: Value, class: Class) -> Result<RpcId, SessionError> {
        let wire = if frozen(method) {
            match self.phase {
                Phase::Handshake { .. } if method == names::INITIALIZE => return Err(SessionError::NotReady(method)),
                _ => params,
            }
        } else {
            self.ready_dialect(method)?.encode_request(method, params)?
        };
        Ok(self.queue_request(method, wire, class))
    }

    /// Sends a notification (for example `stream/credit` for a stream this side receives).
    pub fn notify<N: Notification>(&mut self, params: &N::Params) -> Result<(), SessionError> {
        let dialect = self.ready_dialect(N::NAME)?;
        let params = serde_json::to_value(params).map_err(|source| SessionError::Result { method: N::NAME, source })?;
        let wire = dialect.encode_notification(N::NAME, params)?;
        self.out.push_message(N::INFO.class, &Message::notification(N::NAME, wire))?;
        Ok(())
    }

    /// Answers a request the daemon originated, with a current-shape result or an error.
    pub fn respond(&mut self, id: RpcId, method: &str, outcome: Result<Value, ErrorObject>) -> Result<(), SessionError> {
        let (class, message) = match outcome {
            Ok(result) => {
                let dialect = self.ready_dialect("response")?;
                let class = methods::request_info(method).map_or(Class::Control, |info| info.response_class);
                (class, Message::Response { id, result: dialect.encode_peer_result(method, result)? })
            }
            Err(error) => (Class::Control, Message::Error { id: Some(id), error }),
        };
        self.out.push_message(class, &message)?;
        Ok(())
    }

    /// Decodes the raw result of a request sent with [`ClientSession::request`].
    pub fn decode<M: Method>(&self, wire: Value) -> Result<M::Result, SessionError> {
        let current = if frozen(M::NAME) { wire } else { self.ready_dialect(M::NAME)?.decode_result(M::NAME, wire)? };
        serde_json::from_value(current).map_err(|source| SessionError::Result { method: M::NAME, source })
    }

    /// Opens an outbound data stream (an upload) with the daemon's initial credit.
    pub fn open_stream(&mut self, stream: StreamId, initial_credit: u64) -> Result<(), SessionError> {
        Ok(self.out.open_stream(stream, initial_credit)?)
    }

    /// Queues bytes on an outbound stream; they go out in chunks, under credit, behind control.
    pub fn send_data(&mut self, stream: StreamId, bytes: Vec<u8>) -> Result<(), SessionError> {
        Ok(self.out.send_data(stream, bytes)?)
    }

    /// No more bytes will be queued on `stream`.
    pub fn close_stream(&mut self, stream: StreamId) {
        self.out.close_stream(stream);
    }

    /// Drops a stream and its queued bytes.
    pub fn cancel_stream(&mut self, stream: StreamId) {
        self.out.cancel_stream(stream);
    }

    /// The scheduler, for inspection.
    pub fn scheduler(&self) -> &Scheduler {
        &self.out
    }

    /// The next frame to write, or `None`. Call only when the transport can write it now.
    pub fn poll_transmit(&mut self) -> Option<Vec<u8>> {
        self.out.next_frame()
    }

    /// Reports a completed write, for adaptive chunk sizing.
    pub fn on_written(&mut self, bytes: usize, elapsed: Duration) {
        self.out.on_written(bytes, elapsed);
    }

    /// Feeds received bytes; returns what happened. An error is fatal: close the connection.
    pub fn receive(&mut self, bytes: &[u8]) -> Result<Vec<SessionEvent>, SessionError> {
        self.decoder.push(bytes);
        let mut events = Vec::new();
        while let Some(frame) = self.decoder.next_frame()? {
            match frame {
                Frame::Data { stream, bytes } => events.push(SessionEvent::Data { stream, bytes }),
                Frame::Message(payload) => {
                    let message = Message::from_json(&payload)?;
                    self.on_message(message, &mut events)?;
                }
            }
        }
        Ok(events)
    }

    fn on_message(&mut self, message: Message, events: &mut Vec<SessionEvent>) -> Result<(), SessionError> {
        match message {
            Message::Response { id, result } => self.on_outcome(id, Ok(result), events),
            Message::Error { id: Some(id), error } => self.on_outcome(id, Err(error), events),
            Message::Error { id: None, error } => {
                events.push(SessionEvent::OrphanError(error));
                Ok(())
            }
            Message::Notification { method, params } => {
                let Phase::Ready { dialect, .. } = self.phase else {
                    // Nothing but responses is expected before the handshake; ignore it.
                    return Ok(());
                };
                let inbound = dialect.decode_notification(&method, params)?;
                if let Inbound::StreamCredit(credit) = &inbound {
                    self.out.grant_credit(credit.stream, credit.bytes);
                }
                events.push(SessionEvent::Notification(inbound));
                Ok(())
            }
            Message::Request { id, method, params } => {
                let Phase::Ready { dialect, .. } = self.phase else {
                    let error = ErrorObject::new(ErrorCode::NotInitialized, "the handshake has not completed");
                    self.out.push_message(Class::Control, &Message::Error { id: Some(id), error })?;
                    return Ok(());
                };
                let request = dialect.decode_peer_request(&method, params)?;
                if let PeerRequest::Other { .. } = request {
                    let error = ErrorObject::new(ErrorCode::MethodNotFound, format!("unknown method {method}"));
                    self.out.push_message(Class::Control, &Message::Error { id: Some(id), error })?;
                    return Ok(());
                }
                events.push(SessionEvent::PeerRequest { id, method, request });
                Ok(())
            }
        }
    }

    fn on_outcome(
        &mut self,
        id: RpcId,
        outcome: Result<Value, ErrorObject>,
        events: &mut Vec<SessionEvent>,
    ) -> Result<(), SessionError> {
        let method = self.pending.remove(&id).ok_or_else(|| SessionError::UnknownResponse(id.clone()))?;
        if let Phase::Handshake { id: init } = &self.phase
            && *init == id
        {
            return self.on_initialize(outcome, events);
        }
        if let Err(error) = &outcome
            && error.code == ErrorCode::MethodNotFound
            && let Phase::Ready { features, .. } = &mut self.phase
        {
            features.observe_missing(method);
        }
        events.push(SessionEvent::Response { id, method, outcome });
        Ok(())
    }

    fn on_initialize(&mut self, outcome: Result<Value, ErrorObject>, events: &mut Vec<SessionEvent>) -> Result<(), SessionError> {
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                self.phase = Phase::Failed;
                let older = dialect::describe_unsupported(&error);
                events.push(SessionEvent::NegotiationFailed { error, older });
                return Ok(());
            }
        };
        let result: InitializeResult = serde_json::from_value(result)
            .map_err(|source| SessionError::Result { method: names::INITIALIZE, source })?;
        let negotiated = match dialect::confirm(self.local, &result.protocol) {
            Ok(negotiated) => negotiated,
            Err(error) => {
                self.phase = Phase::Failed;
                let older = Some(error.older_side());
                let error = ErrorObject::new(ErrorCode::ProtocolUnsupported, error.message());
                events.push(SessionEvent::NegotiationFailed { error, older });
                return Ok(());
            }
        };
        let dialect = dialect::select(&negotiated).ok_or(SessionError::NoDialect)?;
        self.phase = Phase::Ready { dialect, negotiated, features: dialect.features().advertised(result.features.as_deref().unwrap_or_default()) };
        events.push(SessionEvent::Ready(ReadyInfo {
            daemon: result.daemon,
            target: result.target,
            daemon_is_older: negotiated.peer_is_older(),
            schema_hash_matches: result.schema_hash == dialect.schema_hash(),
        }));
        Ok(())
    }

    fn ready_dialect(&self, method: &'static str) -> Result<&'static Dialect, SessionError> {
        self.dialect().ok_or(SessionError::NotReady(method))
    }

    fn queue_request(&mut self, method: &'static str, params: Value, class: Class) -> RpcId {
        let id = RpcId::Number(self.next_id);
        self.next_id += 1;
        self.pending.insert(id.clone(), method);
        // A request that does not fit in a frame is a caller bug (pages are bounded by bytes);
        // it is dropped from `pending` so no response is awaited.
        if self.out.push_message(class, &Message::request(id.clone(), method, params)).is_err() {
            self.pending.remove(&id);
        }
        id
    }
}

fn frozen(method: &str) -> bool {
    methods::allowed_before_initialize(method)
}
