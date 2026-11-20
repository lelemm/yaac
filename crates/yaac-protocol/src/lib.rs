//! # yaac-protocol
//!
//! The YAAC wire protocol: the host (hub) ⇄ `yaacd` protocol of docs/03-protocol.md with the
//! mesh additions of docs/10-agent-mesh.md, and the window ⇄ hub attach protocol. Pure and
//! sans-I/O: no async runtime, no sockets, no threads, so it serves a tokio hub and a UI
//! process alike.
//!
//! | Module | What |
//! | --- | --- |
//! | [`frame`] | `u32 LE length · u8 kind · payload` encoder and incremental decoder |
//! | [`schedule`] | two-class (control before bulk) scheduler, adaptive chunks, credit |
//! | [`rpc`] | JSON-RPC 2.0 shapes, error object and codes |
//! | [`methods`] | the method catalogue: names, directions, params and result types |
//! | [`types`] | every wire type (re-exported at the crate root) |
//! | [`dialect`] | protocol versions, negotiation, and the per-version dialect table |
//! | [`session`] | a sans-I/O host-side session: handshake, ids, dialect-driven encode/decode |
//! | [`flows`] | multi-message flows (start a thread, open a thread) over the dialect |
//! | [`attach`] | window ⇄ hub messages (`hub/*`) |
//! | [`codegen`] | schema snapshot, schema hash, TypeScript generation |
//!
//! Forward compatibility: unknown fields are ignored; unknown enum values, capabilities,
//! item kinds, and union variants are preserved and round-trip. Nothing outside
//! [`dialect`] can read a protocol version number.

mod macros;

pub mod attach;
pub mod codegen;
pub mod dialect;
pub mod flows;
pub mod frame;
pub mod methods;
pub mod rpc;
pub mod schedule;
pub mod session;
pub mod types;

pub use types::*;
