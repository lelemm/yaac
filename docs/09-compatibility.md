# 09 — Protocol compatibility

A newer YAAC must keep working with an older `yaacd` that is still busy, and an older YAAC must
keep working with a `yaacd` that another computer has already upgraded. This doc is the design
for that. It is deliberately strict, because compatibility code is the kind that rots first.

**Decided:**

- The UI never forces out an older daemon. It keeps working against it, shows a warning that
  the target is on an older version and will be updated once idle, and offers a button to take
  over immediately.
- Compatibility must not become a copy of the protocol per version, and must not become
  `if version == …` scattered through the code.
- The mechanism is object-oriented: one **dialect** object per protocol version, with **the
  newest version as the base** and each older version overriding only the steps that differed.
- The window is the **last 3 protocol versions**.

## The three rules

1. **Most changes need no version at all.** The protocol is additive by default: unknown fields
   are ignored, unknown enum values are kept and rendered generically, and new abilities are
   announced as capabilities. Adding a field, a method, an event, an item kind, or a capability
   does **not** bump the protocol version.
2. **The newer side adapts.** Whichever peer knows more versions speaks the older peer's
   version. An old build is never expected to understand the future.
3. **A version number is visible in exactly one place.** The handshake picks a dialect. Above
   the dialect layer nothing can read the negotiated version — the code there asks for
   capabilities, as it does for everything else.

The protocol version is bumped only for a change an older peer would *misread*: a removed or
renamed field, a changed meaning, a new required field, a flow that works differently.

## Negotiation

Still once per connection, in the handshake, and never on packets.

```jsonc
// UI → yaacd
{ "id": 1, "method": "initialize",
  "params": { "protocol": { "min": 5, "max": 8 }, "client": { "name": "yaac", "version": "0.9.0" } } }

// yaacd → UI
{ "id": 1,
  "result": { "protocol": { "min": 3, "max": 6, "agreed": 6 }, "schemaHash": "9f2c41d0",
              "daemon": { "build": "0.7.2+a1b2c3", "stateFormat": 4 }, "target": { … } } }
```

- **Agreed version** = the lower of the two `max` values, provided it is not below either `min`.
  Here: 6. The UI then selects its dialect for 6; the daemon speaks 6 natively.
- If the ranges do not overlap, the answer is `protocol_unsupported`, naming both ranges. The
  UI says which side is too old and what to do about it.
- `schemaHash` is the hash of the **frozen baseline** of the agreed version — the snapshot
  taken when that version was introduced, which never changes afterwards. It is a diagnostic:
  a mismatch means one side has a wrong idea of what that version is. It is deliberately not a
  hash of each build's whole current schema, because two builds that speak the same version
  may legitimately differ by an optional field one of them added later.

### The frozen core

A small part of the protocol can never change, because it is what makes negotiation and upgrade
possible between *any* two builds. It is version 1 forever:

- the [frame format](03-protocol.md#framing) and the JSON-RPC message shapes;
- `initialize` and its `protocol` range, and the `protocol_unsupported` error;
- the error object (`code`, `message`, `data`);
- `daemon/status` — build, protocol range, state format, and what it is busy with;
- `daemon/handover` — ask the daemon to step aside, when idle or now.

`daemon/status` and `daemon/handover` are valid even after a failed negotiation. That is what
lets a UI that cannot speak an ancient daemon's protocol still upgrade it. `daemon/handover` is
idempotent, and defined down to what the daemon answers before it closes the connection.

The frozen core is pinned by a **byte-level fixture**: a recorded exchange of bootstrap line,
frames, `initialize`, a failed negotiation, and the two calls permitted after it. Every build,
forever, must reproduce it exactly.

The [stage-0 bootstrap line](02-architecture.md#stage-0) is frozen in the same spirit: it is a
list of `key=value` tokens, new keys may be added, and unknown keys are ignored.

## Dialects

A **dialect** is the object that translates between the wire, at one protocol version, and the
version-free model the rest of the program uses.

```
            ┌──────────────── everything else ────────────────┐
            │  app state · UI · cache · providers · journal   │   knows no version numbers
            └──────────────────────┬──────────────────────────┘
                                   │  domain commands and events
                        ┌──────────▼──────────┐
                        │       Dialect       │   chosen once, at the handshake
                        └──────────┬──────────┘
                                   │  wire messages at the agreed version
                             framing · stream
```

A dialect does four things:

- **encode** a domain command into one or more wire requests;
- **decode** wire responses and notifications into domain results and events;
- run the few **flows** that span several messages (start a thread, open a thread);
- report which wire-level **features** this version has. Missing ones are folded into the
  thread's effective capabilities, so the UI greys a control out for the usual reason — a
  capability is absent — without ever knowing a version was involved.

### Newest is the base

The current protocol is **one complete dialect**, readable top to bottom. Each older version is
a small dialect that inherits from the next-newer one and overrides only what was different
back then.

```ts
// daemon/src/protocol/dialect.ts — the CURRENT protocol (v8). The whole of it lives here.
class Dialect {
  startThread(cmd: StartThread): Promise<Started> {
    const request = this.buildStart(cmd);        // small named steps …
    return this.sendStart(request);              // … are the override points
  }
  protected buildStart(cmd: StartThread): StartRequest { /* … */ }
  protected sendStart(req: StartRequest): Promise<Started> { /* v8: one combined call */ }
  decodeSummary(wire: unknown): ThreadSummary { /* … */ }
}

// daemon/src/protocol/v7.ts — only what v7 did differently from v8
class DialectV7 extends Dialect {
  protected override sendStart(req: StartRequest) { /* create the thread, then the first turn */ }
}

// daemon/src/protocol/v6.ts — only what v6 did differently from v7
class DialectV6 extends DialectV7 {
  override decodeSummary(wire: unknown) { /* the field had its old name */ }
}
```

`DialectV6` behaves like v8, minus the v8 change (inherited from `DialectV7`), minus the v7
change (its own override). Nothing is copied.

Why this direction, rather than "version 1 is the base and each version extends it":

| | Newest is the base | Oldest is the base |
| --- | --- | --- |
| "What does the protocol do today?" | Read one class. | Walk the whole chain. |
| Retiring an old version | Delete its file. Nothing else changes. | Fold it into its successor — a squash. |
| Where the hot path lives | In the base class. | In the most-derived class. |
| Where new work happens | In the base class. Old dialects are frozen and only ever shrink in number. | A new class per version, forever. |

### In Rust

Rust has no class inheritance, and it does not need it here. A dialect is a **table of
functions**, and struct-update syntax is exactly "inherit everything, override these":

```rust
// yaac-protocol/src/dialect/mod.rs
pub struct Dialect {
    pub build_start:    fn(&StartThread) -> StartRequest,
    pub send_start:     fn(&Session, StartRequest) -> BoxFuture<'_, Result<Started>>,
    pub decode_summary: fn(&RawValue) -> Result<ThreadSummary>,
    pub features:       fn() -> FeatureSet,
    // … one entry per overridable step
}

pub const CURRENT: Dialect = Dialect {                       // v8, complete
    build_start: current::build_start,
    send_start: current::send_start,
    decode_summary: current::decode_summary,
    features: current::features,
};
pub const V7: Dialect = Dialect { send_start: v7::send_start, ..CURRENT };
pub const V6: Dialect = Dialect { decode_summary: v6::decode_summary, ..V7 };

/// The only function in the program that looks at a protocol version number.
pub fn select(agreed: u16) -> Option<&'static Dialect> { /* 8 → CURRENT, 7 → V7, 6 → V6, 5 → V5 */ }
```

The flows (`start_thread`, `open_thread`, …) are ordinary functions that take a `&Dialect` and
call its steps. Two rules keep the table honest as versions accumulate:

- **A step takes domain values in and produces wire values out** (or the reverse). Its
  signature never mentions a current wire *struct*, because an old dialect must be able to
  produce a shape the current types no longer have. The wire side of a step is a raw JSON
  value.
- **A step that calls other steps is given the selected `&Dialect`** and calls through it, so
  an older dialect's overrides are honoured all the way down.

The cost is one indirect call per step.

Both sides carry a chain, because both kinds of skew happen:

| Situation | Who adapts | Chain used |
| --- | --- | --- |
| New UI, older daemon still running | The UI | Rust, client role |
| Old UI, daemon already upgraded by another computer | The daemon, for that one connection | TypeScript, server role |

## Making an incompatible change

This is the procedure. Each step is small on purpose.

**0. Check that a bump is needed.** Can the change be additive — a new optional field, a new
method, a new capability? Then do that, and stop here. Most changes stop here.

**1. Bump the version.** Raise `PROTOCOL_VERSION` from *n* to *n+1* and freeze a baseline for
the new version, `protocol/schema/v<n+1>.json`. The baseline of *n* already exists — it was
written when *n* was introduced, and it never changes afterwards. Baselines are generated and
never edited: they are test data, not a second copy of the protocol.

**2. Split, do not rewrite.** If the behaviour that must change sits in the middle of a larger
method, extract exactly that part into its own named step. This is a pure refactoring — no
behaviour changes, all tests pass unchanged — and it is committed on its own.

```ts
// before                                   // after step 2 — same behaviour
openThread(id) {                            openThread(id) {
  const sub = await this.subscribe(id);       const sub = await this.subscribe(id);
  const page = await this.send('thread/read', const page = await this.readFirstPage(id, sub);
                 { id, limit: 50 });          return this.assemble(sub, page);
  return this.assemble(sub, page);          }
}                                           protected readFirstPage(id, sub) {
                                              return this.send('thread/read', { id, limit: 50 });
                                            }
```

**3. Move the old behaviour into the outgoing version's dialect.** Create `DialectV<n>`,
extending the current dialect, overriding only that step with the body it has *right now* —
moved, not retyped. Re-parent the previous old dialect onto the new one (a one-line change).

```ts
class DialectV7 extends Dialect {
  protected override readFirstPage(id, sub) {
    return this.send('thread/read', { id, limit: 50 });            // what v7 did
  }
}
class DialectV6 extends DialectV7 { /* unchanged; was `extends Dialect` */ }
```

**4. Change the step in the current dialect** to the new behaviour.

```ts
protected readFirstPage(id, sub) {
  return this.send('thread/read', { id, atSeq: sub.headSeq, maxBytes: 262144 });   // v8
}
```

**5. Do the mirror change on the other side.** A wire change has a client half (Rust) and a
server half (TypeScript). Each gets the same four steps in its own chain.

**6. Record a fixture.** Add a recorded conversation at version *n* to the compatibility suite
(below). Run it.

**7. Retire what fell out of the window.** If there are now more than three older dialects,
delete the oldest: its dialect file, its schema snapshot, its fixtures. Nothing else references
them.

If the wire shape of a message changed, the old dialect needs the old shape as a type. It
defines **only that message**, next to the override that uses it. Every other message keeps
using the current types.

## What is not allowed

These are the ways compatibility layers go bad. Each is enforced by review, and where possible
by the build.

| Not allowed | Instead |
| --- | --- |
| Reading the negotiated version outside the dialect module. (It is private to that module; the rest of the program cannot name it.) | Ask the dialect for a step, or check a capability. |
| `if (version >= 7) … else …` inside any method, including inside a dialect. | Split the method; override the step. |
| An old dialect that overrides a whole large method to change one line of it. | Split first (step 2), then override the small step. |
| Copying the set of message types into a `v7/` folder. | Old dialects declare only the shapes that actually differed. |
| An old dialect calling into another old dialect by name. | Rely on inheritance order only. |
| Fixing a bug by editing an old dialect's behaviour. | Old dialects reproduce history. Fix the current dialect; an old peer keeps its old bug. |
| A bump to get a feature out. | Capabilities announce features. Bumps are for incompatibility. |

## Tests

Compatibility that is not tested is a rumour.

- **Schema snapshots.** `protocol/schema/v<n>.json` for every supported version. CI diffs the
  current schema against the latest snapshot and fails if it changed incompatibly without a
  version bump.
- **Fixtures.** For each supported version, recorded wire conversations covering every flow.
  Each side's dialect for that version is replayed against them and must produce the same
  domain commands and events as when they were recorded.
- **Real old builds, both ways.** CI keeps the released `yaacd` bundle **and** the released
  hub of each supported protocol version. It runs the current hub against each old daemon, and
  each old hub against the current daemon, end to end on the fake provider — including a
  failed negotiation, a duplicated relay, and a hand-over that lands in the middle of an
  acknowledgement. A recording cannot stand in for an old peer here: it would not reproduce
  that peer's parser, scheduling, or reconnect behaviour.
- **The window.** A test asserts exactly four dialects exist (current plus three), so retiring
  is not forgotten.
- **The ban.** A check fails the build if the negotiated version is referenced outside the
  dialect module.

## Upgrading a daemon without interrupting it

There is at most one serving `yaacd` per user on a target
([architecture](02-architecture.md#one-serving-daemon-per-user)), and daemons only ever move
forward.

**A newer UI meets an older daemon that is running:**

1. Stage 0 reports the build that is serving. The UI installs its own newer build alongside —
   uploaded, verified, **not started** — and attaches to the daemon that is running.
2. The handshake agrees on the older version. The UI selects that dialect and works normally.
3. The target shows a warning: *"This target is running an older YAAC daemon (0.7). It will be
   updated when it is idle."* with a button, **Update now**.
4. The UI calls `daemon/handover { when: "idle" }`. Once the daemon has no running turn, live
   shell command, or pending request, it exits — attached clients do not hold it back. The connection
   drops, the UI reconnects, stage 0 finds nothing serving and starts the newest installed
   build, which migrates the on-disk state. The warning disappears.
5. **Update now** is `daemon/handover { when: "now" }`. Running turns are interrupted, and
   marked as interrupted by an update — never silently — then the same hand-over happens.

Features the older protocol lacks are simply absent until the hand-over: their capabilities are
missing, so their controls are disabled, with the reason shown.

**An older UI meets a newer daemon** (another computer upgraded it): the older UI attaches to
the serving daemon, which selects an older dialect for that one connection. The daemon is never
downgraded: when nothing is serving, stage 0 starts the **newest** installed build, whatever
the connecting UI asked for.

**Outside the window:**

| Situation | What the user sees |
| --- | --- |
| The daemon is older than anything this UI can speak | "The daemon on this target is too old for this YAAC." Wait for it to go idle, or **Update now**. Both work, because `daemon/handover` is in the frozen core. |
| The UI is older than anything the daemon can speak | "This YAAC is too old for the daemon on this target. Update YAAC." Nothing on the target is changed. |

### On-disk state

The journal, registry, and thread index have their own **state format** number, separate from
the protocol version. A newer daemon migrates older state forward when it takes over. There is
no downgrade path and none is needed, because daemons only move forward. Migrations are small,
ordered, individually tested steps, and the previous state is kept until the new daemon has
started successfully.
