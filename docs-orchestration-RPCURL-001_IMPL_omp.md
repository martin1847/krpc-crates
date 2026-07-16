# RPCURL-001 — rpcurl agent-ready: implementation findings

- Owner: omp · Reviewer: codex
- Worktree: `/Users/martin/Garden/middleware/wt-rpcurl001`
- Branch: `feat/rpcurl-agent-ready` (from krpc-crates origin/main @ 6cbbe42)
- Scope: `crates/rpcurl` only. No wire changes, no krpc server changes.
- Commits LOCAL; not pushed.

## r2 — codex REQUEST-CHANGES addressed (amended into the single commit)

All r1 review findings resolved; `cargo test` (22 unit + 15 integration),
`cargo clippy --all-targets` (also `-D print_stdout -D print_stderr`), and
`cargo build -r` stay green.

- **blocking #1 — `--json` CLI parse errors bypassed the JSON contract.** clap
  exits before `run()`. Fixed in `main.rs`: `main` now pre-scans argv for `--json`
  and uses `Cli::try_parse()`; `handle_parse_error()` routes genuine parse errors
  through the JSON error renderer (`{"error":{"kind":"usage",...}}`, exit 2) when
  `--json` is present, keeps clap's human message otherwise, and lets
  `--help`/`--version` print to stdout with exit 0 in both modes. New real-process
  tests: unknown option (JSON), subcommand/arg conflict (JSON), non-JSON human
  path, help/version exit 0.
- **blocking #2 — default human `Out::Bytes` output regressed.** Restored the exact
  pre-r1 format on the human path: `🌱` + hand-formatted `"data<bytes>":[..]`
  (the normalized `"data":[..]` envelope is now `--json`-only). Regression test
  `bytes_human_mode_matches_historical_format` asserts the byte-exact string.
- **advisory #4 (prompt item 3) — reached-but-misbehaving server misclassified.**
  New `CliError::Protocol` (exit 4, kind `"protocol"`): HTTP non-2xx, non-UTF8
  body, and unparseable ApiMeta now map here instead of `Connect` (exit 3). Pure
  connection/transfer failures remain `Connect`. README exit table + `--json`
  contract updated; tests cover 404 and malformed-body → exit 4.
- **advisory #3 — no direct regression for the `Out::Error` preimage defect.**
  Extracted `response_error()` in `main.rs` and unit-tested that `Out::Error` maps
  to `Remote{exit 1}` (not stdout/exit 0) and `Out::Json` is success. A full
  live-gRPC server test was not added: the `krpc` dep is built with only the `clt`
  feature (no server surface), so standing up a real KRPC gRPC responder in-test
  would require enabling an out-of-scope feature. The defect fix is covered by the
  classifier unit test + the output-routing and fd-split integration tests.

## TL;DR

All four deliverables landed and verified:

1. Error semantics fixed — every error path writes to real stderr (fd2) with a
   non-zero, machine-branchable exit code; stdout stays pure data.
2. `--json` mode — pure JSON on stdout (no emoji), errors as a JSON object on stderr.
3. Introspection trio — `discover`, `schema`, `example` over `GET /agent/discover`.
4. Tests — 20 unit + 9 integration, incl. a live localhost HTTP round-trip for
   discover/schema/example and real-process fd-separation assertions.

`cargo test` green · `cargo clippy --all-targets` clean (also clean under
`-D clippy::print_stdout -D clippy::print_stderr`, matching the workspace lint
intent) · `cargo build -r --locked -p rpcurl` succeeds.

## 1. Field claims — verified

Repro against current `main` (pre-fix, `target/debug/rpcurl`), unreachable endpoint:

```
$ rpcurl http://127.0.0.1:59999/app/Svc/method -d '{}' 2>&1 | cat
Error: tonic::transport::Error(Transport, ConnectError(ConnectError("tcp connect error", ...ConnectionRefused...)))
--- rc=0 (pipe) ---
$ ... 2>/dev/null            # STDOUT=[]   (empty)
$ ... 2>&1 >/dev/null        # STDERR=[Error: tonic::transport::Error(...)]
$ ... >/dev/null 2>/dev/null; echo $?   # rc=1
```

Findings, corrected against the ground-truth claims:

- **"rc may even be 0 on error."** TWO distinct surfaces, only one was truly rc=0:
  - **Transport/`Status` errors** (`?`-propagated) already reached **stderr** with
    **rc 1** via Rust's default `Termination`. The `rc=0` seen in the field was the
    **shell pipeline artifact**: `cmd 2>&1 | cat` makes `$?` report `cat`'s exit (0),
    not rpcurl's. rpcurl's own code was 1. (So "errors vanish under `2>&1 | cat`"
    did not reproduce here — the message was present on the merged stream; the
    misleading part is `$?`.) Message quality was still poor: raw `Error: <Debug>`,
    not JSON, and the actionable cause buried in `Debug`.
  - **Application errors** (`Out::Error`, i.e. a *successful* gRPC response carrying
    `code != 0`) were printed to **stdout** with the ❌ emoji and `main` returned
    `Ok` → **rc 0**. **This is the real, confirmed rc=0-on-error bug**: a business
    failure was byte-for-byte indistinguishable from success for a machine consumer
    (wrong stream + wrong exit code).
- **Emoji contamination.** Confirmed: 🍀 / ❌ / 🌱 were unconditionally prefixed to
  stdout, so machine consumers had to strip them.
- **Zero introspection.** Confirmed: no discover/schema surface existed.

## 2. What changed

New module layout under `crates/rpcurl/src`:

- `error.rs` — `CliError { Usage, Connect, Remote{code,msg} }` + stable exit-code
  taxonomy + `chain()` (flattens an error's `source()` chain so connect failures
  show `transport error: tcp connect error: Connection refused (os error 61)`
  instead of the bare `transport error`).
- `output.rs` — `Mode { Human, Json }`; `render_success`/`render_error` write only
  through explicit `Write` handles (never `print!`/`eprintln!`, so the streams are
  injectable and the `print_stdout`/`print_stderr` clippy lints never fire).
- `discover.rs` — `ApiMeta` model (serde mirror of `tech.krpc.common.meta.*`),
  plain-HTTP fetch of `/agent/discover` via the hyper stack already in the tree,
  JSON-Schema-ish derivation, and `discover`/`schema`/`example` renderers.
- `args.rs` — clap `Cli` with optional subcommands + flattened invoke args
  (`args_conflicts_with_subcommands`); pure, panic-free helpers returning `CliError`.
- `main.rs` — `#[tokio::main] async fn main() -> ExitCode`; dispatch; maps every
  failure to stderr + a non-zero `ExitCode`.

### Exit-code contract (documented in README + `error.rs`)

| code | meaning |
|------|---------|
| 0 | success (data on stdout) |
| 1 | remote error: server reachable, returned an error (gRPC `Status` or `Out::Error`) |
| 2 | usage error: bad CLI input (url / json / file / args) |
| 3 | connect error: could not establish a connection |

### `--json`

Pure JSON on stdout, no emoji. Errors → single-line JSON object on **stderr**:
`{"error":{"kind":"connect|usage|remote","msg":"...","code":<grpc-code?>}}`.
Default human mode keeps 🍀 / 🌱 / ❌.

### Introspection

- `discover <base-url>` → `{app, sdkVersion, apiVersion, services:[{name,description,
  methods:[{name,path:"Svc/method",arg,res,doc}]}]}`.
- `schema <base-url> <Service/method>` → `{method, doc, input:<schema>, output:<schema>}`.
  RpcResult/Optional wrappers are unwrapped on the output side; jakarta validation
  constraints (`@NotBlank`→required+minLength, `@Size`/`@Length`, `@Min`/`@Max`,
  `@Pattern`, `@Email`, `@Positive`, …) and `@Doc` are folded into the schema;
  unmapped constraints are preserved under `x-constraints`. Recursive type graphs
  are broken with an `x-ref` stub.
- `example <base-url> <Service/method>` → placeholder input skeleton derived from
  the input schema (kept — cheap; ~30 lines).

The discover endpoint is fetched over **http only** (task spec: "plain HTTP on the
krpc http port"); an `https://` base is rejected with a usage error.

## 3. Behavior changes (approved point)

- **Error fd fix (the deliverable):** `Out::Error` now goes to **stderr with exit 1**
  instead of stdout with exit 0. Transport/`Status` errors keep stderr but now emit a
  clean, cause-chained message (and JSON in `--json`) instead of `Error: <Debug>`.
- **Verbose diagnostics moved stdout → stderr.** `-v` dumps (url, request json,
  headers, response headers) now go to stderr so stdout is pure data even with `-v`.
  Non-default flag; improves machine consumption.
- Success stdout format for the **default** (non-`--json`, non-verbose) path is
  unchanged: `🍀`/`🌱` prefix + pretty envelope.

## 4. Tests

- `src/args.rs` — url splitting (http/https/malformed), json-input validation,
  malformed-`-H` warning path.
- `src/output.rs` — `--json` purity (no emoji, parses as JSON), human emoji kept,
  bytes rendering, error-object shape.
- `src/discover.rs` — fixture parse, method lookup + errors, discover listing,
  schema (`@NotBlank`→required+minLength, RpcResult unwrap, int64 format), example
  skeleton, origin stripping.
- `tests/cli.rs` — spawns the **compiled binary**: connect error → stdout empty +
  stderr non-empty + **exit 3**; `--json` error is a JSON object on stderr; bad
  url/json → **exit 2**; and a throwaway localhost HTTP server serving the fixture
  proves discover/schema/example end-to-end with **pure JSON on stdout**.

## 5. Live-server verification — honest status

**Not verified against a live krpc server.** `gradle :examples:quickstart:run`
fails to boot in this environment: `java.lang.ClassNotFoundException:
tech.krpc.annotation.UnsafeWeb` (a krpc-side build breakage — the annotation module
is not on the quickstart runtime classpath). Fixing that is out of scope (no krpc
changes). Discover/schema were therefore verified against:

- a **hand-crafted fixture** (`crates/rpcurl/tests/fixtures/apimeta_quickstart.json`)
  built faithfully from the real Java class shapes (`AgentDiscoverHandler`,
  `ApiMeta`/`Api`/`Method`/`PropertyType`/`Dto`/`Property`/`Anno`, and the quickstart
  `HelloService`/`HelloRequest`/`HelloReply`), and
- a **live localhost HTTP round-trip** in the integration tests + manual smoke
  (a python server serving the fixture on `/agent/discover`), which exercises the
  full fetch → parse → schema → render → stdout path.

**Fixture assumptions to re-check against a live server** (parser is defensive
about all of them):

1. `Method.arg/res.rawType` may be name-only with fields in the top-level `dtos`
   closure, OR carry fields inline — the resolver handles both (inline first, then
   `dtos` lookup).
2. `res` may be `RpcResult<Payload>` or already unwrapped to `Payload` — schema is
   correct either way (RpcResult/Optional unwrap is a no-op when absent).
3. Annotation JSON uses `{name, properties}` with `name` = FQN (per `Anno.java`);
   `@Doc` carries `properties.value`. If a live server serialized annotations
   differently, `doc`/`required` extraction would need adjustment — everything else
   is annotation-independent.

## 6. Dependencies added

Introspection needs a plain-HTTP client; reused the hyper stack already present via
tonic (no heavyweight new dep like reqwest): `hyper` (client/http1), `hyper-util`
(client-legacy/http1/tokio), `http-body-util`, `bytes`, and `serde` (derive, for the
`ApiMeta` model). All were already in `Cargo.lock`; `--locked` build passes.

## 7. Commands

```
cargo test -p rpcurl
cargo clippy --all-targets -- -D clippy::print_stdout -D clippy::print_stderr
cargo build -r --locked -p rpcurl
```
