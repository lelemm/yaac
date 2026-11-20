# yaacd

The YAAC daemon on each target (docs/02-architecture.md#daemon-lifecycle). TypeScript, Node 22,
no runtime dependencies; bundled into one file, `dist/yaacd.mjs`.

```sh
npm install
npm run build       # dist/yaacd.mjs
npm run typecheck
npm test            # builds, compiles the tests, runs them (node --test)
```

```sh
yaacd serve [--idle-timeout <ms>]   # the per-user daemon on a user-only Unix socket
yaacd connect                       # stdio ⇄ socket bridge; starts the daemon detached if needed
yaacd version
```

Environment: `YAAC_HOME` (default `~/.yaac`), `YAAC_SOCKET` (override the socket path),
`YAAC_IDLE_TIMEOUT_MS` (default 600000; `0` never), `YAAC_FAKE_PROVIDER=1`
(see `src/providers/fake/README.md`). Uploads go to `$TMPDIR/yaac-<user>/uploads`.

Layout: `src/protocol/` frames, canonical JSON, scheduler, negotiation and the dialect chain
(the only place a protocol version is read; `generated/` is generated); `src/session/` the
daemon, connections, journal, index, idempotency, uploads; `src/providers/` the provider
contract (`types.ts`), the registry, and the fake provider.

Note: `--no-turbofan` is passed to node in the scripts: the Node 22.22.3 build on the
development machine crashes intermittently in V8's optimising compiler.
