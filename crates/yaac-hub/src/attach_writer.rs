//! Outbound attach frames. One writer holds at most one encoded frame outside the queues.
use tokio::sync::mpsc;
use yaac_protocol::StreamId;
use yaac_protocol::frame::{FrameError, encode_data_frame};
use yaac_protocol::message_chunks;
use yaac_protocol::rpc::{ErrorCode, ErrorObject, Message};

#[derive(Debug)]
pub(crate) enum Packet {
    Rpc(Message),
    Data(StreamId, Vec<u8>),
}

impl Packet {
    pub(crate) fn encode(self, buffer: &mut Vec<u8>) -> Result<(), FrameError> {
        buffer.clear();
        match self {
            Self::Rpc(msg) => {
                let frames = match message_chunks::encode(&msg.to_json(), Some(0)) {
                    Ok(frames) => frames,
                    Err(error) => {
                        let id = match msg {
                            Message::Response { id, .. } => Some(id),
                            Message::Error { id, .. } => id,
                            _ => return Err(error),
                        };
                        let response = Message::Error { id, error: ErrorObject::new(ErrorCode::LimitExceeded, "result exceeds the attach message limit") };
                        message_chunks::encode(&response.to_json(), None)?
                    }
                };
                for frame in frames {
                    buffer.extend_from_slice(&frame);
                }
                Ok(())
            }
            Self::Data(stream, bytes) => encode_data_frame(stream, &bytes, buffer),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Sender {
    ordinary: mpsc::UnboundedSender<Packet>,
    bulk: mpsc::UnboundedSender<Packet>,
    terminal: mpsc::UnboundedSender<Packet>,
}

impl Sender {
    pub(crate) fn send(&self, msg: Message) -> Result<(), ()> {
        let queue = match &msg {
            Message::Request { method, .. } | Message::Notification { method, .. }
                if method.starts_with("hub/terminal/") || method.starts_with("hub/voice/") || method.starts_with("hub/voiceCall/") || method == "stream/credit" =>
            {
                &self.terminal
            }
            Message::Request { method, params, .. }
                if method == yaac_protocol::attach::names::REQUEST
                    && params["method"]
                        .as_str()
                        .and_then(yaac_protocol::methods::request_info)
                        .is_some_and(|info| info.request_class == yaac_protocol::schedule::Class::Bulk) =>
            {
                &self.bulk
            }
            _ => &self.ordinary,
        };
        queue.send(Packet::Rpc(msg)).map_err(|_| ())
    }

    pub(crate) fn bulk_reply(&self, msg: Message) -> Result<(), ()> {
        self.bulk.send(Packet::Rpc(msg)).map_err(|_| ())
    }

    /// Stream setup and subsequent events on that stream share FIFO ordering.
    pub(crate) fn terminal_reply(&self, msg: Message) -> Result<(), ()> {
        self.terminal.send(Packet::Rpc(msg)).map_err(|_| ())
    }

    pub(crate) fn data(&self, stream: StreamId, bytes: Vec<u8>) -> Result<(), ()> {
        self.terminal.send(Packet::Data(stream, bytes)).map_err(|_| ())
    }
}

pub(crate) struct Receiver {
    ordinary: mpsc::UnboundedReceiver<Packet>,
    bulk: mpsc::UnboundedReceiver<Packet>,
    terminal: mpsc::UnboundedReceiver<Packet>,
    burst: usize,
}

pub(crate) fn channel() -> (Sender, Receiver) {
    let (ordinary, o) = mpsc::unbounded_channel();
    let (bulk, b) = mpsc::unbounded_channel();
    let (terminal, t) = mpsc::unbounded_channel();
    (
        Sender { ordinary, terminal, bulk },
        Receiver {
            ordinary: o,
            bulk: b,
            terminal: t,
            burst: 0,
        },
    )
}

impl Receiver {
    pub(crate) async fn recv(&mut self) -> Option<Packet> {
        if self.burst >= 8 {
            self.burst = 0;
            if let Ok(packet) = self.ordinary.try_recv() {
                return Some(packet);
            }
        }
        tokio::select! {
            biased;
            Some(packet) = self.terminal.recv() => { self.burst += 1; Some(packet) }
            Some(packet) = self.ordinary.recv() => { self.burst = 0; Some(packet) }
            Some(packet) = self.bulk.recv() => { self.burst = 0; Some(packet) }
            else => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yaac_protocol::rpc::RpcId;

    #[tokio::test]
    async fn interactive_data_overtakes_bulk_and_interleaves_with_control() {
        let (tx, mut rx) = channel();
        tx.bulk_reply(Message::Response {
            id: RpcId::Number(1),
            result: json!({}),
        })
        .unwrap();
        tx.send(Message::notification("notice", json!({}))).unwrap();
        for key in 0..10 {
            tx.data(StreamId(9), vec![key]).unwrap();
        }
        drop(tx);
        for key in 0..10 {
            if key == 8 {
                assert!(matches!(rx.recv().await, Some(Packet::Rpc(Message::Notification { .. }))));
            }
            assert!(matches!(rx.recv().await, Some(Packet::Data(StreamId(9), data)) if data == [key]));
        }
        assert!(matches!(rx.recv().await, Some(Packet::Rpc(Message::Response { .. }))));
        assert!(rx.recv().await.is_none());
    }
}
