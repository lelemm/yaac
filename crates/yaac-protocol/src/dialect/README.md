# Dialects — making an incompatible protocol change

This directory is the only place in the program that can read a protocol version number
(`ProtocolVersion`, `ProtocolRange`, `AgreedProtocol`, and `Negotiated` keep it in private
fields). Everything above it asks the selected `Dialect` to do a step, or checks a capability.
The binding design is [docs/09-compatibility.md](../../../../docs/09-compatibility.md); this
file restates its procedure in terms of this code.

- `mod.rs` — versions, `negotiate`, `confirm`, the `Dialect` table of step functions,
  `CURRENT`, the `ALL` list (newest first), and `select`, the only function that compares a
  version number.
- `current.rs` — the current protocol, complete: one function per step.
- `v<n>.rs` — one per older supported version (none yet: `PROTOCOL_VERSION` is 1). Each holds
  only the steps that differed, and only the wire types that differed.

Steps take domain values in and produce raw `serde_json::Value` out (or the reverse); a step
that calls other steps takes the selected `&Dialect` and calls through it, so an older
dialect's overrides are honoured all the way down.

## The procedure

**0. Check that a bump is needed.** If the change can be additive — a new optional field, a
new method, a new enum value or union variant, a new capability — make it, regenerate
(`cargo run -p yaac-protocol --bin yaac-protocol-gen`), and stop. The
`the_frozen_baseline_exists_and_the_current_schema_is_an_additive_change_of_it` test proves it
was additive. Most changes stop here.

**1. Freeze.** In `mod.rs`, bump `PROTOCOL_VERSION` from `ProtocolVersion(n)` to
`ProtocolVersion(n + 1)` (`SUPPORTED` follows: the last 3 versions besides the current one).
Run `cargo run -p yaac-protocol --bin yaac-protocol-gen -- --freeze` to write the new
version's frozen baseline `protocol/schema/v<n+1>.json`; `build.rs` turns it into
`codegen::SCHEMA_HASH_V<n+1>`, which `CURRENT.schema_hash` must now name. Then run the generator once more (no flags) so the TypeScript
`SCHEMA_HASH` follows. Baselines are generated, never edited, and never rewritten once
released.

**2. Split, do not rewrite.** If the behaviour that must change is inside a larger step in
`current.rs`, extract exactly that part into a new step: add a field to `Dialect`, a function
in `current.rs`, an entry in `CURRENT`, and make the larger step call it through the
`&Dialect` it receives. Pure refactoring — every test passes unchanged. Commit on its own.

**3. Move the old behaviour into the outgoing version's dialect.** Create `v<n>.rs` with the
step's body *as it is now* (moved, not retyped), and in `mod.rs`:

```rust
mod vN;
pub const VN: Dialect = Dialect { version: ProtocolVersion(n), schema_hash: crate::codegen::SCHEMA_HASH_VN, the_step: vN::the_step, ..CURRENT };
```

Re-parent the previous old dialect onto it — `VN_1` changes from `..CURRENT` to `..VN`, a
one-line change — and add `&VN` to `ALL` after `&CURRENT`. If the wire shape of a message
changed, `v<n>.rs` defines **only that message's** old shape, next to the override that uses
it.

**4. Change the step in `current.rs`** to the new behaviour, and update the Rust types (the
version-free model) to match.

**5. Do the mirror change on the other side.** The server half lives in `yaacd`'s TypeScript
dialect chain (`daemon/src/protocol/`), which gets the same four steps.

**6. Record a fixture.** Add a recorded conversation at version *n* to the compatibility suite
and run it. The frozen core (`protocol/schema/frozen-core-v1.txt`) never changes.

**7. Retire what fell out of the window.** If `ALL` now holds more than four dialects
(current plus three), delete the oldest: its `v<k>.rs`, its line in `ALL`, its
`protocol/schema/v<k>.json`, and its fixtures. Re-parent nothing — the next-oldest already
inherits from a newer one. The test `exactly_one_dialect_per_supported_version_and_at_most_four`
fails until this is done.

## Not allowed

- Reading the negotiated version outside this directory (privacy forbids it; `tests/ban.rs`
  scans the workspace as well).
- `if version …` inside any step, including here. Split the step; override it.
- An old dialect overriding a large step to change one line of it. Split first (step 2).
- Copying the message types into a `v<n>/` folder. Old dialects declare only what differed.
- One old dialect calling another by name. Rely on struct-update order only.
- Fixing a bug by editing an old dialect. Old dialects reproduce history.
- A bump to ship a feature. Capabilities announce features (see `FeatureSet`: a version that
  lacks a wire feature withholds the matching capabilities).
