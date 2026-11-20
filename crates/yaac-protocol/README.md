# yaac-protocol

The YAAC wire protocol, as Rust types: the host ⇄ `yaacd` protocol of
[docs/03-protocol.md](../../docs/03-protocol.md) with the mesh additions of
[docs/10-agent-mesh.md](../../docs/10-agent-mesh.md), plus the window ⇄ hub attach protocol.
**These types are the source of truth**; the TypeScript in `daemon/src/protocol/generated/` is
generated from them and never edited by hand.

Pure and sans-I/O: no async runtime, no sockets, no threads. A tokio hub and a UI process use
it the same way — feed it bytes, ask it what to write.

## Regenerating

```sh
cargo run -p yaac-protocol --bin yaac-protocol-gen              # rewrite the TypeScript bindings
cargo run -p yaac-protocol --bin yaac-protocol-gen -- --check   # exit 1 if anything is stale or incompatible
```

Run it after changing any wire type. `cargo test` fails when the committed TypeScript is
stale (`tests/generated.rs`).

`protocol/schema/v<n>.json` is the **frozen baseline** of protocol version *n*, written once
when the version is introduced (`-- --freeze`) and never edited; `schemaHash` is computed from
it by `build.rs`. Later changes must be additive — the test
`the_frozen_baseline_exists_and_the_current_schema_is_an_additive_change_of_it` checks that.
An incompatible change follows [src/dialect/README.md](src/dialect/README.md). (Before v1 is
first released the baseline may still be re-frozen with `-- --freeze --force`, then the
generator run again so the TypeScript hash follows.)

To type-check the generated TypeScript under `strict` as part of the tests:
`YAAC_TSC=1 cargo test -p yaac-protocol --test generated` (needs Node and, the first time,
network for `npx`).

## Public API outline

| Module | Main items |
| --- | --- |
| `frame` | `Frame { Message(Vec<u8>), Data { stream, bytes } }`, `encode_frame`, `encode_message_frame`, `encode_data_frame`, `FrameDecoder { push, next_frame, decode }`, `FrameError`, `MAX_FRAME_LEN` (1 MiB, counts kind + payload) |
| `schedule` | `Scheduler { push_frame, push_message, open_stream, send_data, grant_credit, close_stream, cancel_stream, next_frame, on_written }`, `Class { Control, Bulk }`, `ReceiveWindow { on_received, on_consumed }`, `CHUNK_MIN`/`CHUNK_MAX` |
| `rpc` | `Message { Request, Response, Error, Notification }` (`to_json`, `from_json`), `RpcId`, `ErrorObject`, `ErrorCode` (open, snake_case strings) |
| `methods` | `names::*` (every method name), one marker per method implementing `Method { NAME, Params, Result, INFO }` or `Notification`, `REQUESTS`, `NOTIFICATIONS`, `Direction`, `allowed_before_initialize`, `request_info` |
| `types` (re-exported at the root) | ids (`ThreadId`, `TurnId`, `ItemId`, `InstanceId`, `ProviderKind`, `ModelId`, `EffortId`, `ModeId`, `RequestId`, `UploadId`, `OpId`, `MessageId`, `TargetId`, `TargetName`, `ClientId`, `SyncToken`, `PageCursor`, `Timestamp`, `TargetPath`, `StreamId`, `Epoch`, `Seq`, `JournalPosition`, `Address`); `Capability`, `Support`, `ModelCapability`; `ProviderDescriptor`, `ModelInfo`, `EffortInfo`, `ModeInfo`, `StoreIdentity`, `ProviderStatus`, `BackendKind`, `ProviderInstanceConfig`; `ThreadSummary`, `ThreadState`, `Control`, `OriginKind`, `ProjectRef`, `WorktreeRef`, `ThreadEffective`, `AppliedSettings`; `InputPart`; `Item` = `ItemBody` (`userMessage` … `meshWait`, `Unknown`); `RequestItem` with `RequestBody` (`approval`/`question`/`form`), `RequestState`, `RequestResponse`, `RequestResolution`; `ThreadEventParams` with `ThreadEvent`; subscribe/read/ack shapes; `ThreadSync*`; uploads; `fs/*`, `project/resolve`; session (`InitializeParams/Result`, `DaemonStatusResult`, `DaemonHandover*`); mesh (`MeshMessage`, `Receipt`, `MeshRelayParams`, `MeshDeliverParams`, `MeshQuery`/`MeshQueryResult`, `MeshRosterParams`, `MeshPolicy`, `MeshBudget*`); and a `*Params`/`*Result` type for every method |
| `dialect` | `PROTOCOL_VERSION`, `SUPPORTED`, `ProtocolRange`, `AgreedProtocol`, `negotiate`, `confirm`, `Negotiated { peer_is_older, local_is_older, answer, diagnostic_label }`, `NegotiationError { older_side, message, to_error_object }`, `describe_unsupported`, `Dialect` (steps), `CURRENT`, `select`, `FeatureSet`, `Inbound`, `PeerRequest` |
| `session` | `ClientSession { new, request::<M>, request_value, notify::<N>, respond, decode::<M>, open_stream, send_data, poll_transmit, on_written, receive, fold_capabilities }`, `SessionEvent { Ready, NegotiationFailed, Response, Notification, PeerRequest, Data, OrphanError }` |
| `flows` | `StartThread` → `start_thread` / `finish_start_thread`; `OpenThread` → `OpenThreadFlow { start, on_response }` → `Opened { subscribed, snapshot }`; `read_older_page`, `finish_page` |
| `attach` | `names::*` (`hub/hello`, `hub/shutdown`, `hub/targets/*`, `hub/target/state`, `hub/request`, `hub/notification`), `HelloParams/Result`, `TargetEntry`, `TargetTransport` (`local`/`ssh`/`wsl`), `ConnectionState`, `RoutedRequest`, `RoutedNotification` |
| `codegen` | `current_schema`, `SCHEMA_HASH_V1`, `schema_hash`, `generated_files`, `stale_files`, `compat::check` |

## Rules this crate enforces

- **Forward compatible.** Unknown fields are ignored. Every enum is open: unknown strings are
  kept (`Capability::Unknown("…")`), unknown union variants are kept as the raw object
  (`ItemBody::Unknown(UnknownVariant)`), and both re-encode exactly.
- **No version outside the dialect.** Version numbers live in private fields of
  `dialect` types, with no accessor, equality, or ordering (`compile_fail` doctests in
  `src/dialect/mod.rs`); `tests/ban.rs` also scans the workspace.
- **No version on frames or messages.** The frame format and JSON-RPC shapes are the frozen
  core, pinned byte for byte by `protocol/schema/frozen-core-v1.txt`.
- **Canonical JSON.** Envelope members in a fixed order, every object inside them with sorted
  keys. Do not enable `serde_json`'s `preserve_order` feature anywhere in the workspace — it
  would change the bytes, and the frozen-core test would fail.
- **No model ids.** Nothing about models is in this crate; tests use `fake-model`.
