# RPCURL-001 r1 review

Reviewed `6903f00` against `origin/main` (`6cbbe42`). Scope is confined to
`crates/rpcurl`, its lockfile, and the implementation note; no wire,
release, or publish change was found. The hand-crafted fixture is honestly
marked “Not verified against a live krpc server”, and its consumed field
names/nesting agree with `tech.krpc.common.meta.{ApiMeta,Api,Method,PropertyType,Dto,Property,Anno}` as serialized by Jackson.

## Findings

1. **blocking — `crates/rpcurl/src/main.rs:18` — `--json` parse failures bypass the JSON error contract.**

   `Cli::parse()` exits inside clap before `mode` is selected and before the
   stderr renderer at lines 24–31 runs. Reproduced with
   `target/debug/rpcurl --json --not-a-real-option`: exit is 2 and stdout is
   empty, but stderr begins `error: unexpected argument ...` and includes
   human usage text, not the documented single-line JSON object. The same
   happens for a subcommand/argument conflict. This contradicts the
   `--json` contract in `crates/rpcurl/README.md:53-55`; in contrast, a
   post-parse malformed URL does correctly produce JSON on stderr.

   Suggested fix: use clap's fallible parse API, determine `--json` from the
   raw arguments (or a minimal pre-parse), and route parse errors through the
   same JSON error renderer while preserving normal human help/version
   behavior. Add real-process coverage for unknown options and command/arg
   conflicts in JSON mode.

2. **blocking — `crates/rpcurl/src/output.rs:24-37` — default human bytes output changed beyond the approved error routing fix.**

   `success_value()` now unconditionally creates a `"data"` field for
   `Out::Bytes`, and `render_success()` uses that value for both modes. In the
   parent (`6cbbe42:crates/rpcurl/src/main.rs:58`), the default human bytes
   output was `🌱` followed by a `"data<bytes>"` field. The new default human
   output is still prefixed `🌱`, but exposes `"data"` instead. That breaks
   existing human-mode consumers and conflicts with the claimed unchanged
   default output. No test covers the old field name.

   Suggested fix: keep the established `data<bytes>` representation on the
   human path exactly as before; use the normalized `data` JSON envelope only
   for `--json`. Add a regression assertion for the default `Out::Bytes`
   rendering.

3. **advisory — `crates/rpcurl/tests/cli.rs:45-68` — the fd/exit regression test does not exercise the historical `Out::Error` failure.**

   The only real-process error fixture uses an unreachable endpoint. On old
   main, transport errors already reached stderr with a non-zero process exit;
   this test would fail old main because it now requires exit 3, but not
   because it detects the reported stdout/exit-0 `Out::Error` bug. There is no
   test that makes a gRPC response carrying `OutputProto { out: Error(...) }`
   and asserts fd2, empty stdout, and exit 1.

   Suggested fix: run a small tonic test server that returns application-level
   `Out::Error`, then assert the process stream split and exit code. This is
   the red regression test for the actual preimage defect.

4. **advisory — `crates/rpcurl/src/discover.rs:442-446` — reachable HTTP failures are labelled and exited as connection failures.**

   A server that returns HTTP 404/500, a malformed ApiMeta body, or a non-UTF8
   body has been reached, but these paths return `CliError::Connect` and exit
   3. README line 51 defines 3 as “could not reach the endpoint”; that is
   inaccurate for an HTTP response. The stream split remains correct, but an
   agent cannot distinguish a network failure from a server/protocol failure.

   Suggested fix: add an HTTP/protocol remote-error classification (or map it
   to the documented remote class) and cover non-2xx plus invalid-body
   responses.

## Verification

- `cargo test -p rpcurl`: 20 unit and 9 integration tests passed. The first
  sandboxed run could not bind loopback sockets; the same command was rerun
  where loopback binding is allowed and passed.
- `cargo clippy --all-targets`: passed without warnings.
- Read every new CLI error dispatch path. Connect, tonic `Status`, malformed
  URL, and JSON parse errors that reach `run()` are rendered to fd2 with a
  non-zero exit. Clap parse errors are fd2/non-zero but violate JSON-mode
  error formatting as described above.

## Verdict

REQUEST-CHANGES — JSON-mode argument errors are not machine-parseable, and default human bytes output is not backward compatible.
