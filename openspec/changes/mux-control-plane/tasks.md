# Tasks

## 1. Daemon and protocol

- [x] 1.1 Finish `radar daemon`, private socket/state path resolution, bounded versioned JSON-lines, ping/refusals, trusted directories and clean shutdown; exercise real temporary sockets.
- [x] 1.2 Finish atomic private request records, same-ID deduplication/content-conflict checks, expiry, persistence across restart and unknown outcomes; serialize/refuse conflicting mutations across methods rather than by method alone.
- [x] 1.3 Add the normalized backend interface and Herdr adapter, reusing existing transport/decoders where possible; publish explicit capabilities and keep all Herdr wire details adapter-owned.

## 2. Observation and guarded primitives

- [x] 2.1 Serve inventory, foreground evidence and focus; prove fixture parity with the direct runtime and exercise the actual daemon socket with a fake backend.
- [x] 2.2 Serve close with fresh frozen-identity/containment checks, partial-effect/unknown outcomes and no managed-owner bypass; prove same-ID at-most-once and no effectful retry after disconnect.
- [x] 2.3 Serve normalized create/split, literal text/key input and bounded output reads; validate inputs, return created identities and test real behavior only on disposable surfaces.
- [x] 2.4 Add the state/session/display metadata reporting bridge for extension callers, preserving source/sequence/TTL and declaring unsupported operations explicitly; no registry or inferred authority.

## 3. Radar integration and migration

- [x] 3.1 Add the daemon runtime client and explicit selection/startup fallback diagnostic. Keep managed close/restart and confirmation unchanged; never fall back directly after possible daemon dispatch.
- [x] 3.2 Publish the protocol fixture/reference, daemon/client usage and extension-porting examples. Document current capabilities, uncertainty semantics and deferred tmux/session-awareness work.

## 4. Verification

- [x] 4.1 Independently run fmt, Clippy with warnings denied, locked tests/build, both PTY smokes, checked Nix build and strict spec validation. Record real-socket and disposable-backend evidence with provenance and limits.

Deferred: production tmux backend, publisher registry, resume-command inference, lead recovery, restart batches and agent parent/child topology. The backend-neutral interface is their seam, not their implementation.
