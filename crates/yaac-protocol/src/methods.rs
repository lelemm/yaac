//! The method catalogue: every request and notification of the YAAC protocol, in one place.
//!
//! * [`names`] has every method name as a constant.
//! * Each method has a marker type here implementing [`Method`] (requests) or
//!   [`Notification`], tying the name to its params and result types.
//! * [`REQUESTS`] and [`NOTIFICATIONS`] describe them all at run time (used by the schema and
//!   TypeScript generators).

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::schedule::Class;
use crate::types as t;

/// Who sends a method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// The host (hub) sends it to `yaacd`.
    HostToDaemon,
    /// `yaacd` sends it to the host.
    DaemonToHost,
    /// Either side.
    Both,
}

impl Direction {
    /// Wire-style name, for generated code.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::HostToDaemon => "hostToDaemon",
            Direction::DaemonToHost => "daemonToHost",
            Direction::Both => "both",
        }
    }
}

/// A request method.
pub trait Method {
    /// The wire name.
    const NAME: &'static str;
    /// Params type.
    type Params: Serialize + DeserializeOwned + JsonSchema;
    /// Result type.
    type Result: Serialize + DeserializeOwned + JsonSchema;
    /// Static facts about the method.
    const INFO: MethodInfo;
}

/// A notification method.
pub trait Notification {
    /// The wire name.
    const NAME: &'static str;
    /// Params type.
    type Params: Serialize + DeserializeOwned + JsonSchema;
    /// Static facts about the notification.
    const INFO: NotificationInfo;
}

/// Static facts about a request method.
#[derive(Clone, Copy, Debug)]
pub struct MethodInfo {
    /// Wire name.
    pub name: &'static str,
    /// Who sends it.
    pub direction: Direction,
    /// Part of the frozen core: never changes, valid before (and after a failed) `initialize`
    /// where noted in [`allowed_before_initialize`].
    pub frozen: bool,
    /// Class of the request.
    pub request_class: Class,
    /// Class of the response (bulk for pages).
    pub response_class: Class,
    /// Whether the request changes state and so carries an `opId`.
    pub state_changing: bool,
    /// One-line purpose.
    pub doc: &'static str,
    /// Name of the params type in the schema.
    pub params_type: fn() -> String,
    /// Name of the result type in the schema.
    pub result_type: fn() -> String,
    /// Adds the params schema to a generator.
    pub params_schema: fn(&mut SchemaGenerator) -> Schema,
    /// Adds the result schema to a generator.
    pub result_schema: fn(&mut SchemaGenerator) -> Schema,
    /// Decodes and re-encodes params JSON (tests).
    #[doc(hidden)]
    pub roundtrip_params: fn(serde_json::Value) -> Result<serde_json::Value, String>,
    /// Decodes and re-encodes result JSON (tests).
    #[doc(hidden)]
    pub roundtrip_result: fn(serde_json::Value) -> Result<serde_json::Value, String>,
}

/// Static facts about a notification.
#[derive(Clone, Copy, Debug)]
pub struct NotificationInfo {
    /// Wire name.
    pub name: &'static str,
    /// Who sends it.
    pub direction: Direction,
    /// Class.
    pub class: Class,
    /// One-line purpose.
    pub doc: &'static str,
    /// Name of the params type in the schema.
    pub params_type: fn() -> String,
    /// Adds the params schema to a generator.
    pub params_schema: fn(&mut SchemaGenerator) -> Schema,
    /// Decodes and re-encodes params JSON (tests).
    #[doc(hidden)]
    pub roundtrip_params: fn(serde_json::Value) -> Result<serde_json::Value, String>,
}

fn roundtrip<T: Serialize + DeserializeOwned>(value: serde_json::Value) -> Result<serde_json::Value, String> {
    let typed: T = serde_json::from_value(value).map_err(|e| e.to_string())?;
    serde_json::to_value(&typed).map_err(|e| e.to_string())
}

fn schema_of<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<T>()
}

fn name_of<T: JsonSchema>() -> String {
    T::schema_name().into_owned()
}

macro_rules! catalogue {
    (
        requests {
            $(
                $(#[doc = $doc:literal])*
                $marker:ident = $wire:literal as $const:ident :
                    $params:ty => $result:ty, $dir:ident, request $rclass:ident, response $sclass:ident
                    $(, $flag:ident)* ;
            )+
        }
        notifications {
            $(
                $(#[doc = $ndoc:literal])*
                $nmarker:ident = $nwire:literal as $nconst:ident : $nparams:ty, $ndir:ident, $nclass:ident ;
            )+
        }
    ) => {
        /// Every method name.
        pub mod names {
            $(
                #[doc = concat!("`", $wire, "`")]
                pub const $const: &str = $wire;
            )+
            $(
                #[doc = concat!("`", $nwire, "`")]
                pub const $nconst: &str = $nwire;
            )+
        }

        $(
            $(#[doc = $doc])*
            #[derive(Clone, Copy, Debug)]
            pub struct $marker;

            impl Method for $marker {
                const NAME: &'static str = $wire;
                type Params = $params;
                type Result = $result;
                const INFO: MethodInfo = MethodInfo {
                    name: $wire,
                    direction: Direction::$dir,
                    frozen: catalogue!(@has frozen $($flag)*),
                    request_class: Class::$rclass,
                    response_class: Class::$sclass,
                    state_changing: catalogue!(@has op $($flag)*),
                    doc: crate::methods::first_line(concat!($($doc, "\n"),*)),
                    params_type: name_of::<$params>,
                    result_type: name_of::<$result>,
                    params_schema: schema_of::<$params>,
                    result_schema: schema_of::<$result>,
                    roundtrip_params: roundtrip::<$params>,
                    roundtrip_result: roundtrip::<$result>,
                };
            }
        )+

        $(
            $(#[doc = $ndoc])*
            #[derive(Clone, Copy, Debug)]
            pub struct $nmarker;

            impl Notification for $nmarker {
                const NAME: &'static str = $nwire;
                type Params = $nparams;
                const INFO: NotificationInfo = NotificationInfo {
                    name: $nwire,
                    direction: Direction::$ndir,
                    class: Class::$nclass,
                    doc: crate::methods::first_line(concat!($($ndoc, "\n"),*)),
                    params_type: name_of::<$nparams>,
                    params_schema: schema_of::<$nparams>,
                    roundtrip_params: roundtrip::<$nparams>,
                };
            }
        )+

        /// Every request method, in catalogue order.
        pub const REQUESTS: &[MethodInfo] = &[$(<$marker as Method>::INFO),+];
        /// Every notification, in catalogue order.
        pub const NOTIFICATIONS: &[NotificationInfo] = &[$(<$nmarker as Notification>::INFO),+];
    };
    (@has $want:ident) => { false };
    (@has frozen frozen $($rest:ident)*) => { true };
    (@has op op $($rest:ident)*) => { true };
    (@has $want:ident $other:ident $($rest:ident)*) => { catalogue!(@has $want $($rest)*) };
}

/// The first non-empty line of a doc string, trimmed (const, for `MethodInfo::doc`).
pub(crate) const fn first_line(doc: &'static str) -> &'static str {
    let bytes = doc.as_bytes();
    let mut start = 0;
    while start < bytes.len() && (bytes[start] == b' ' || bytes[start] == b'\n') {
        start += 1;
    }
    let mut end = start;
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }
    let (_, tail) = bytes.split_at(start);
    let (line, _) = tail.split_at(end - start);
    match std::str::from_utf8(line) {
        Ok(line) => line,
        Err(_) => "",
    }
}

catalogue! {
    requests {
        // ── Session ────────────────────────────────────────────────────────────────────
        /// Must be the first message: negotiate the protocol version; returns daemon and target facts.
        Initialize = "initialize" as INITIALIZE:
            t::InitializeParams => t::InitializeResult, HostToDaemon, request Control, response Control, frozen;
        /// Build, protocol range, state format, and what the daemon is busy with.
        DaemonStatus = "daemon/status" as DAEMON_STATUS:
            t::DaemonStatusParams => t::DaemonStatusResult, HostToDaemon, request Control, response Control, frozen;
        /// Ask the daemon to step aside for a newer build, when idle or now. Idempotent.
        DaemonHandover = "daemon/handover" as DAEMON_HANDOVER:
            t::DaemonHandoverParams => t::DaemonHandoverResult, HostToDaemon, request Control, response Control, frozen;
        /// Liveness and latency.
        Ping = "ping" as PING:
            t::PingParams => t::PingResult, HostToDaemon, request Control, response Control;
        /// Redacted daemon and provider diagnostics. Never message content.
        DiagnosticsGet = "diagnostics/get" as DIAGNOSTICS_GET:
            t::DiagnosticsGetParams => t::DiagnosticsGetResult, HostToDaemon, request Control, response Bulk;

        // ── Providers ──────────────────────────────────────────────────────────────────
        /// All provider instances on the target, each with a full descriptor.
        ProviderList = "provider/list" as PROVIDER_LIST:
            t::ProviderListParams => t::ProviderListResult, HostToDaemon, request Control, response Control;
        /// Force a re-probe of one instance or all.
        ProviderRefresh = "provider/refresh" as PROVIDER_REFRESH:
            t::ProviderRefreshParams => t::Empty, HostToDaemon, request Control, response Control;
        /// Store or reset a user’s model recommendation override.
        ModelBestUseSet = "provider/modelBestUse/set" as PROVIDER_MODEL_BEST_USE_SET:
            t::ModelBestUseSetParams => t::Empty, HostToDaemon, request Control, response Control, op;
        /// The stored instance configurations, including implicit defaults.
        ProviderInstanceList = "providerInstance/list" as PROVIDER_INSTANCE_LIST:
            t::ProviderInstanceListParams => t::ProviderInstanceListResult, HostToDaemon, request Control, response Control;
        /// Add a provider instance.
        ProviderInstanceAdd = "providerInstance/add" as PROVIDER_INSTANCE_ADD:
            t::ProviderInstanceAddParams => t::ProviderInstanceResult, HostToDaemon, request Control, response Control, op;
        /// Change a provider instance (a patch).
        ProviderInstanceUpdate = "providerInstance/update" as PROVIDER_INSTANCE_UPDATE:
            t::ProviderInstanceUpdateParams => t::ProviderInstanceResult, HostToDaemon, request Control, response Control, op;
        /// Remove a provider instance.
        ProviderInstanceRemove = "providerInstance/remove" as PROVIDER_INSTANCE_REMOVE:
            t::ProviderInstanceRemoveParams => t::Empty, HostToDaemon, request Control, response Control, op;

        // ── Threads ────────────────────────────────────────────────────────────────────
        /// Incremental thread index.
        ThreadSync = "thread/sync" as THREAD_SYNC:
            t::ThreadSyncParams => t::ThreadSyncResult, HostToDaemon, request Control, response Bulk;
        /// Create a thread and send its first message, as one operation.
        ThreadStart = "thread/start" as THREAD_START:
            t::ThreadStartParams => t::ThreadStartResult, HostToDaemon, request Control, response Control, op;
        /// A page of items, newest first, with the journal position the snapshot is valid at.
        ThreadRead = "thread/read" as THREAD_READ:
            t::ThreadReadParams => t::ThreadReadResult, HostToDaemon, request Control, response Bulk;
        /// Start live events for a thread, replaying from a journal position.
        ThreadSubscribe = "thread/subscribe" as THREAD_SUBSCRIBE:
            t::ThreadSubscribeParams => t::ThreadSubscribeResult, HostToDaemon, request Control, response Control;
        /// Stop live events for a thread.
        ThreadUnsubscribe = "thread/unsubscribe" as THREAD_UNSUBSCRIBE:
            t::ThreadUnsubscribeParams => t::Empty, HostToDaemon, request Control, response Control;
        /// Report how far this client has durably got in a thread's journal.
        ThreadAck = "thread/ack" as THREAD_ACK:
            t::ThreadAckParams => t::Empty, HostToDaemon, request Control, response Control;
        /// Create a new thread from an existing one, with its first message.
        ThreadFork = "thread/fork" as THREAD_FORK:
            t::ThreadForkParams => t::ThreadForkResult, HostToDaemon, request Control, response Control, op;
        /// What a fork would send and where. Changes nothing.
        ThreadForkPreview = "thread/forkPreview" as THREAD_FORK_PREVIEW:
            t::ThreadForkPreviewParams => t::ThreadForkPreviewResult, HostToDaemon, request Control, response Bulk;
        /// Rename a thread (under `threadRename`).
        ThreadRename = "thread/rename" as THREAD_RENAME:
            t::ThreadRenameParams => t::Empty, HostToDaemon, request Control, response Control, op;
        /// Archive a thread (under `threadArchive`).
        ThreadArchive = "thread/archive" as THREAD_ARCHIVE:
            t::ThreadArchiveParams => t::Empty, HostToDaemon, request Control, response Control, op;
        /// Delete a thread (under `threadDelete`).
        ThreadDelete = "thread/delete" as THREAD_DELETE:
            t::ThreadDeleteParams => t::Empty, HostToDaemon, request Control, response Control, op;
        /// Continue here a thread another program holds (under `threadTakeOver`, D140): the
        /// holder's evidence no longer makes it read-only, until a different holder appears.
        ThreadTakeOver = "thread/takeOver" as THREAD_TAKE_OVER:
            t::ThreadTakeOverParams => t::Empty, HostToDaemon, request Control, response Control, op;
        /// Run the harness’s manual context compaction (under `compact`).
        ThreadCompact = "thread/compact" as THREAD_COMPACT:
            t::ThreadCompactParams => t::Empty, HostToDaemon, request Control, response Control, op;
        /// List a thread under another project or worktree on the same target (D144); with
        /// `threadMoveWorkingDirectory`, its working directory follows from the next turn.
        ThreadMove = "thread/move" as THREAD_MOVE:
            t::ThreadMoveParams => t::ThreadMoveResult, HostToDaemon, request Control, response Control, op;
        /// Set YAAC's own settings of a thread (the agents policy, a hold). A patch.
        ThreadUpdate = "thread/update" as THREAD_UPDATE:
            t::ThreadUpdateParams => t::ThreadUpdateResult, HostToDaemon, request Control, response Control, op;
        /// Create a new thread from an existing one without a first message.
        ThreadForkOnly = "thread/forkOnly" as THREAD_FORK_ONLY:
            t::ThreadForkOnlyParams => t::ThreadForkOnlyResult, HostToDaemon, request Control, response Control, op;
        /// A byte range of one item's full output (a tool call's output beyond its inline head).
        ItemRead = "item/read" as ITEM_READ:
            t::ItemReadParams => t::ItemReadResult, HostToDaemon, request Control, response Bulk;
        /// Unarchive a thread (under `threadArchive`), with the harness's own unarchive.
        ThreadUnarchive = "thread/unarchive" as THREAD_UNARCHIVE:
            t::ThreadArchiveParams => t::Empty, HostToDaemon, request Control, response Control, op;
        /// The threads the host keeps (pinned or muted there): never cut by the discovery caps.
        ThreadKeep = "thread/keep" as THREAD_KEEP:
            t::ThreadKeepParams => t::Empty, HostToDaemon, request Control, response Control;
        /// Find a thread by id, asking the harness when the index does not have it.
        ThreadLookup = "thread/lookup" as THREAD_LOOKUP:
            t::ThreadLookupParams => t::ThreadLookupResult, HostToDaemon, request Control, response Control;
        /// A page of conversations beyond the discovery caps, newest first.
        ThreadOlder = "thread/older" as THREAD_OLDER:
            t::ThreadOlderParams => t::ThreadOlderResult, HostToDaemon, request Control, response Bulk;
        /// Read, and optionally change, the target's discovery caps.
        DiscoveryCapsMethod = "discovery/caps" as DISCOVERY_CAPS:
            t::DiscoveryCapsParams => t::DiscoveryCapsResult, HostToDaemon, request Control, response Control;

        // ── Turns ──────────────────────────────────────────────────────────────────────
        /// Send a user message to an existing thread.
        TurnStart = "turn/start" as TURN_START:
            t::TurnStartParams => t::TurnStartResult, HostToDaemon, request Control, response Control, op;
        /// Stop a turn, by turn id.
        TurnInterrupt = "turn/interrupt" as TURN_INTERRUPT:
            t::TurnInterruptParams => t::Empty, HostToDaemon, request Control, response Control;
        /// Add input to the running turn (under `steer`).
        TurnSteer = "turn/steer" as TURN_STEER:
            t::TurnSteerParams => t::Empty, HostToDaemon, request Control, response Control, op;
        /// Replace a queued user message before it is delivered.
        TurnQueueEdit = "turn/queueEdit" as TURN_QUEUE_EDIT:
            t::TurnQueueEditParams => t::Empty, HostToDaemon, request Control, response Control, op;
        /// Cancel a queued user message before it is delivered.
        CommandLogs = "command/logs" as COMMAND_LOGS:
            t::CommandLogsParams => t::CommandLogsResult, HostToDaemon, request Control, response Control;
        CommandStop = "command/stop" as COMMAND_STOP:
            t::CommandStopParams => t::Empty, HostToDaemon, request Control, response Control, op;
        TurnQueueCancel = "turn/queueCancel" as TURN_QUEUE_CANCEL:
            t::TurnQueueCancelParams => t::Empty, HostToDaemon, request Control, response Control, op;
        /// Send a queued user message now ("Send now"): out of the queue, with `now` mechanics.
        TurnQueuePromote = "turn/queuePromote" as TURN_QUEUE_PROMOTE:
            t::TurnQueueCancelParams => t::TurnStartResult, HostToDaemon, request Control, response Control, op;
        /// Answer a pending approval, question, or form.
        RequestRespond = "request/respond" as REQUEST_RESPOND:
            t::RequestRespondParams => t::Empty, HostToDaemon, request Control, response Control, op;

        // ── Uploads ────────────────────────────────────────────────────────────────────
        /// Declare an upload; returns its id and data stream, or that the content exists.
        UploadBegin = "upload/begin" as UPLOAD_BEGIN:
            t::UploadBeginParams => t::UploadBeginResult, HostToDaemon, request Control, response Control;
        /// All bytes sent: verify the hash.
        UploadEnd = "upload/end" as UPLOAD_END:
            t::UploadEndParams => t::UploadEndResult, HostToDaemon, request Control, response Control;
        /// Abort an upload; the partial file is removed.
        UploadCancel = "upload/cancel" as UPLOAD_CANCEL:
            t::UploadCancelParams => t::Empty, HostToDaemon, request Control, response Control;
        /// A bounded read of a stored attachment.
        UploadRead = "upload/read" as UPLOAD_READ:
            t::UploadReadParams => t::UploadReadResult, HostToDaemon, request Control, response Bulk;

        // ── Filesystem (read-only) ─────────────────────────────────────────────────────
        /// List a directory on the target.
        FsList = "fs/list" as FS_LIST:
            t::FsListParams => t::FsListResult, HostToDaemon, request Control, response Control;
        /// Find files under a working directory on the target.
        FsSearch = "fs/search" as FS_SEARCH:
            t::FsSearchParams => t::FsSearchResult, HostToDaemon, request Control, response Control;
        /// The git identity of a directory.
        ProjectResolve = "project/resolve" as PROJECT_RESOLVE:
            t::ProjectResolveParams => t::ProjectResolveResult, HostToDaemon, request Control, response Control;

        // ── Skills ─────────────────────────────────────────────────────────────────────
        /// The skills one instance offers for a working directory (under `skills`).
        SkillsList = "skills/list" as SKILLS_LIST:
            t::SkillsListParams => t::SkillsListResult, HostToDaemon, request Control, response Control;

        // ── Projects ───────────────────────────────────────────────────────────────────
        /// The projects the user created on this target.
        ProjectList = "project/list" as PROJECT_LIST:
            t::ProjectListParams => t::ProjectListResult, HostToDaemon, request Control, response Control;
        /// Create a project: a folder and a name. Moves every thread inside it under it.
        ProjectCreate = "project/create" as PROJECT_CREATE:
            t::ProjectCreateParams => t::ProjectResult, HostToDaemon, request Control, response Control, op;
        /// Rename a project.
        ProjectRename = "project/rename" as PROJECT_RENAME:
            t::ProjectRenameParams => t::ProjectResult, HostToDaemon, request Control, response Control, op;
        /// Delete a project; its folder and threads are untouched.
        ProjectDelete = "project/delete" as PROJECT_DELETE:
            t::ProjectDeleteParams => t::Empty, HostToDaemon, request Control, response Control, op;

        // ── Agent mesh ─────────────────────────────────────────────────────────────────
        /// Target → hub: carry a message to another target; answered once durably queued.
        MeshRelay = "mesh/relay" as MESH_RELAY:
            t::MeshRelayParams => t::MeshReceiptResult, DaemonToHost, request Control, response Control;
        /// Hub → target: put a message into a recipient's mailbox; answered once on disk.
        MeshDeliver = "mesh/deliver" as MESH_DELIVER:
            t::MeshDeliverParams => t::MeshReceiptResult, HostToDaemon, request Control, response Control;
        /// Hub → source target: a relayed message's receipt changed.
        MeshReceipt = "mesh/receipt" as MESH_RECEIPT:
            t::MeshReceiptParams => t::Empty, HostToDaemon, request Control, response Control;
        /// Recipient target → hub: final harness handoff outcome for a relayed message.
        MeshDeliveryReceipt = "mesh/deliveryReceipt" as MESH_DELIVERY_RECEIPT:
            t::MeshReceiptParams => t::Empty, DaemonToHost, request Control, response Control;
        /// A tool call that needs another target, routed through the hub (both directions).
        MeshQuery = "mesh/query" as MESH_QUERY:
            t::MeshQueryParams => t::MeshQueryResult, Both, request Control, response Bulk;
        /// Hub → target: the target's own identity and the roster of targets.
        MeshRoster = "mesh/roster" as MESH_ROSTER:
            t::MeshRosterParams => t::Empty, HostToDaemon, request Control, response Control;
        /// Read, and optionally change, the mesh policy, limits, and budgets.
        MeshPolicyMethod = "mesh/policy" as MESH_POLICY:
            t::MeshPolicyParams => t::MeshPolicyResult, HostToDaemon, request Control, response Control;
        /// Read or continue an orchestration's budget.
        MeshBudgetMethod = "mesh/budget" as MESH_BUDGET:
            t::MeshBudgetParams => t::MeshBudgetResult, HostToDaemon, request Control, response Control, op;
        /// Target → hub → the root's target: spending counted against a root on another target.
        MeshUsage = "mesh/usage" as MESH_USAGE:
            t::MeshUsageParams => t::MeshUsageResult, Both, request Control, response Control;

        // ── Pull requests (D119–D121) ──────────────────────────────────────────────────
        /// The pull requests whose head is a thread's branch, and what can be done with them.
        ForgePrs = "forge/prs" as FORGE_PRS:
            t::ForgePrsParams => t::ForgePrsResult, HostToDaemon, request Control, response Control;
        /// What the Create PR modal needs: default branch, branches, prefilled text, push state.
        ForgePrepare = "forge/prepare" as FORGE_PREPARE:
            t::ForgePrepareParams => t::ForgePrepareResult, HostToDaemon, request Control, response Control;
        /// Create a pull request from a thread's branch, pushing it first when needed.
        ForgePrCreate = "forge/pr/create" as FORGE_PR_CREATE:
            t::ForgePrCreateParams => t::ForgePrCreateResult, HostToDaemon, request Control, response Control, op;
        /// One pull request in full: description, reviewers, checks.
        ForgePrGet = "forge/pr/get" as FORGE_PR_GET:
            t::ForgePrGetParams => t::ForgePrGetResult, HostToDaemon, request Control, response Control;

        // ── Rate limits (D161) ─────────────────────────────────────────────────────────
        /// Each `rateLimits` instance's account and usage windows, as last reported; `refresh`
        /// reads the harness again (at most every five minutes per instance).
        ProviderRateLimits = "provider/rateLimits" as PROVIDER_RATE_LIMITS:
            t::ProviderRateLimitsParams => t::ProviderRateLimitsResult, HostToDaemon, request Control, response Control;
    }
    notifications {
        /// An instance's descriptor changed.
        ProviderUpdated = "provider/updated" as PROVIDER_UPDATED:
            t::ProviderUpdatedParams, DaemonToHost, Control;
        /// A thread summary changed (including threads created outside YAAC).
        ThreadUpdated = "thread/updated" as THREAD_UPDATED:
            t::ThreadUpdatedParams, DaemonToHost, Control;
        /// A thread was positively reported gone.
        ThreadRemoved = "thread/removed" as THREAD_REMOVED:
            t::ThreadRemovedParams, DaemonToHost, Control;
        /// One journaled thread event.
        ThreadEvent = "thread/event" as THREAD_EVENT:
            t::ThreadEventParams, DaemonToHost, Control;
        /// The project list changed (created, renamed, deleted); carries every project.
        ProjectUpdated = "project/updated" as PROJECT_UPDATED:
            t::ProjectListResult, DaemonToHost, Control;
        /// The discovery caps changed.
        DiscoveryUpdated = "discovery/updated" as DISCOVERY_UPDATED:
            t::DiscoveryCapsResult, DaemonToHost, Control;
        /// The receiver of a data stream grants more credit.
        StreamCredit = "stream/credit" as STREAM_CREDIT:
            t::StreamCreditParams, Both, Control;
        /// A branch's pull requests changed (or were asked again after a push or commit).
        ForgePrsChanged = "forge/prs/changed" as FORGE_PRS_CHANGED:
            t::ForgePrsResult, DaemonToHost, Control;
        /// An instance's account or rate-limit windows changed (D161).
        ProviderRateLimitsUpdated = "provider/rateLimits/updated" as PROVIDER_RATE_LIMITS_UPDATED:
            t::InstanceRateLimits, DaemonToHost, Control;
        /// A root's budget ran out, was freed, or started or stopped holding messages back
        /// (D164). The hub relays it to every window.
        MeshBudgetChanged = "mesh/budget/changed" as MESH_BUDGET_CHANGED:
            t::MeshBudgetResult, DaemonToHost, Control;
    }
}

/// Whether a method may be sent before `initialize` has succeeded — `initialize` itself and
/// the two frozen daemon-management calls, which stay valid even after a failed negotiation.
pub fn allowed_before_initialize(method: &str) -> bool {
    matches!(method, names::INITIALIZE | names::DAEMON_STATUS | names::DAEMON_HANDOVER)
}

/// Looks up a request method by name.
pub fn request_info(method: &str) -> Option<&'static MethodInfo> {
    REQUESTS.iter().find(|m| m.name == method)
}

/// Looks up a notification by name.
pub fn notification_info(method: &str) -> Option<&'static NotificationInfo> {
    NOTIFICATIONS.iter().find(|m| m.name == method)
}
