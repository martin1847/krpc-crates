# krpc-crates — Agent Guide

Rust workspace of KRPC components and demos. Notably hosts the **`rpcurl` CLI**
(`crates/rpcurl`) — the **active** command-line client for the whole ecosystem
(the Java `java-rpcurl` is deprecated). `rpcurl` ships to **GitHub Releases, not
crates.io**; the `krpc` native-smoke CI pulls the latest linux-musl build from
there. Capability cluster: umbrella `docs/modules/polyglot-runtimes.md`.

## Scope

- FOR: the `rpcurl` CLI client, shared Rust crates, and Rust demos over the KRPC
  wire; keeping them wire-compatible with `krpc`.
- NOT FOR: defining protocol/contract semantics (those follow `krpc`); a Java-side
  client library.
- Maintained-only: work happens on concrete pressure (a real bug / a real consumer
  need), not speculative feature parity.

## Ecosystem rules

- Wire compatibility originates in `krpc`; this repo follows the wire, never forks
  the protocol, never leads a wire change (NS-2).
- Tier-2 maintained: touch only on a concrete need, not for speculative parity
  (NS-8).
- Long-term direction: the KRPC umbrella workspace `docs/NORTH_STAR.md` (cite
  principles by NS-ID when relevant).

## Build & test

- `cargo build -r` — release build; targets built per-platform, e.g.
  `cargo build -r --target x86_64-unknown-linux-musl` (not verified).
- `cargo test` (not verified).
- Release artifacts land under `dist/` and are published to GitHub Releases.

## Discipline

- Behavior changes hide behind a flag, default OFF. No drive-by refactors or
  format churn.
- Secrets, internal hostnames/IPs, topology never enter the committed tree, logs,
  or docs.
- Commits stay local until the owner approves a push. No AI signature lines.
