//! The window's one connection: to the hub, over the attach protocol.

use yaac_protocol::rpc::Message;

/// A bidirectional, non-blocking channel of attach-protocol messages to the hub
/// (`yaac_protocol::attach`). The real implementation is the hub client; tests and `--demo`
/// use [`crate::demo::DemoHub`].
///
/// Neither operation may block: `send` queues, `poll` returns what has already arrived.
pub trait HubLink {
    /// Queue a message for the hub.
    fn send(&mut self, message: Message);
    /// The next message from the hub, if one has arrived.
    fn poll(&mut self) -> Option<Message>;
}

/// A link that is not connected: it drops what is sent and never receives anything. Used
/// before the hub client exists, and in tests of pure state.
#[derive(Debug, Default)]
pub struct DetachedLink;

impl HubLink for DetachedLink {
    fn send(&mut self, _message: Message) {}
    fn poll(&mut self) -> Option<Message> {
        None
    }
}
