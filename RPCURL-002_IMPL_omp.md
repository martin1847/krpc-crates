# RPCURL-002 implementation findings (omp)

Implements the nine 1.1.0 items of `DESIGN-RPCURL-002.md` §3.1/§5 in `crates/rpcurl`,
version bumped `1.0.0 → 1.1.0`. One implementation commit atop design `ba555de`
(design commit left intact). Local only; no push, no tag/release (GitHub Release +
native-smoke unpin are the maintainer's, per §3.3).

## Gates

- `cargo test -p rpcurl` → **69 passed** (36 unit + 33 integration), 0 failed.
- `cargo clippy --all-targets -p rpcurl -- -D warnings` → **0 warnings** (print_stdout/
  print_stderr discipline held: all output goes through injected `Write` handles).
- `cargo build -r -p rpcurl` → OK.

## Per-item delivery vs §5 table

| # | P0 item | Status | Where |
|---|---|---|---|
| 1 | `--format human\|json\|auto`, **default `auto`** | ✅ done | `args.rs` `Format`; `main.rs::resolve_mode` (`IsTerminal`) |
| 2 | Error record `{code,message,violations?}`+`type`+`cli` | ✅ done | `output.rs::render_error`; `error.rs::code_str`/`grpc_code_name` |
| 3 | Diagnostics JSONL (warnings + error framing) | ✅ done | `output.rs::render_warning`; `main.rs` `-H` warnings |
| 4 | `-c` hard-error + `-b` cookie-send | ✅ done | `args.rs` (`cookie`=`-b`, hidden `cookie_jar`=`-c`); `check_cookie_jar` |
| 5 | `-d @file` / `-d @-` / drop `-f` / one-body | ✅ done | `args.rs::read_data`; `-f` removed |
| 6 | exit set `0–5` (add `5`=timeout) | ✅ done | `error.rs::CliError::Timeout` (exit 5) |
| 7 | `--max-time` + `--connect-timeout` (`-m` reused) | ✅ done | `args.rs`; `main.rs::invoke` `tokio::time::timeout` |
| 8 | `--client-id`/`--client-meta` rename, `-H` colon, `--oauth2-bearer` | ✅ done | `args.rs` |
| 9 | did-you-mean + error→schema `cli.hint` | ✅ done | `discover.rs::did_you_mean`; `main.rs::with_hint` |

All nine shipped. Nothing deferred from P0. (P1/not-planned untouched, per design.)

## Red-first captures (against the pre-change binary at `ba555de`)

Probed the current binary before implementing; each failed the new contract:

| Behavior | Current (RED) | New (1.1.0) |
|---|---|---|
| piped invoke error | `❌ transport error: …` (human) | JSON record on stderr |
| `-c 'tk=x'` | accepted as cookie send (proceeds) | exit 2, message points at `-b` |
| machine error shape | `{"error":{"kind":"connect","msg":…}}` | `{"type":"error","code":"CONNECT","message":…,"cli":{…}}` |
| `-d @-` | `failed to parse json < @- >` (literal) | reads stdin |

These are now locked by tests: `auto_default_piped_error_is_agent002_json`,
`cookie_jar_c_is_hard_error_pointing_at_b`, `output::error_json_mode_is_agent002_envelope`,
`data_at_stdin_is_read_and_validated`.

## Test-contract migration (RPCURL-001 → RPCURL-002)

RPCURL-001 tests that encoded a *replaced* contract were updated to the new contract
(surviving contracts were not weakened):

- `output.rs::error_json_mode_is_object_with_kind` → **replaced** by
  `error_json_mode_is_agent002_envelope` (old `{"error":{kind,msg,code}}` → new triplet
  `{type,code,message,cli}`; asserts no legacy `msg`).
- `cli.rs::json_mode_error_is_json_object_on_stderr` → **replaced** by
  `format_json_error_is_agent002_envelope` (+ new `auto_default_piped_error_is_agent002_json`);
  `--json` → `--format json`; `v["error"]["kind"]` → `v["cli"]["kind"]` + `v["type"]`.
- `cli.rs::json_mode_unknown_option_is_json_error_on_stderr` /
  `json_mode_arg_subcommand_conflict_is_json_error` → **renamed** `format_json_*`, envelope
  assertions migrated to `cli.kind`; `--json` → `--format json`.
- `cli.rs::non_json_unknown_option_stays_human_exit_2` → **replaced** by
  `format_human_unknown_option_stays_human_exit_2` (needs explicit `--format human` now that
  piped default is JSON) **plus** new `auto_pipe_unknown_option_is_json_error`.
- `cli.rs::discover_human_lists_methods` → needs `--format human` (piped default is JSON).
- `cli.rs::{discover_json,schema_json,example,discover_http_404,help_and_version}` → `--json`
  → `--format json`; `discover_http_404` envelope assertion → `v["cli"]["kind"]`.
- `args.rs::malformed_header_becomes_warning_not_error` → survives; input changed to a
  no-separator token (`noseparator`) since `=` is now a valid separator.

Surviving contracts kept as-is: stdout/stderr routing, success envelope shape, `Out::Error`
→ exit 1, byte-payload human format, help/version exit 0.

New tests added for 1.1.0 contracts: auto-default JSON (invoke + discover + parse error),
`-c` hard error, `-d @-` stdin (valid + invalid), `--max-time` timeout exit 5, did-you-mean
suggestion, `--format human` decoration, `--oauth2-bearer`/colon-header parsing (unit),
timeout/negative-timeout validation (unit), `with_hint` (unit).

## Design amendments made this commit (rule 1: doc ↔ code no drift)

Small deviations discovered during implementation, folded into `DESIGN-RPCURL-002.md`:

1. **`W_HEADER_SEPARATOR` dropped** (§2.3 registry, §1.2). Compat is waived, so `:` and `=`
   are *both* first-class permanently — there is no deprecation, hence no separator warning.
   Only `W_HEADER_MALFORMED` (no separator at all) is emitted in P0.
2. **Header names are lowercased** (§1.2). tonic metadata keys must be ascii-lowercase;
   lowercasing also lets an agent paste a curl `Content-Type:`-style header verbatim.
   Colon is tried first, then `=`, so a value may contain `:` (e.g. `Bearer a:b`).
3. **Verbose (`-v`) is outside the JSONL contract** (§2.3). `-v` dumps are plain-text human
   debug aids; agents use machine mode without `-v`. Structured warnings/errors remain JSONL.
4. **r5 advisory** applied: §0/§3.1 "everything ships" → "every item marked 1.1.0 ships".

## Implementation notes (no doc change needed)

- **One-body rule** (§1.1) is enforced for free: with `-f` removed, the only body source is
  `-d`, and clap already rejects a repeated `-d` with a usage error (exit 2). No custom code.
- **`-c` bare** (no value) yields clap's generic "requires a value" usage error (still exit
  2); the helpful "use `-b`" message fires for `-c <value>` (the common trap case).
- **Timeouts** wrap the network phase with `tokio::time::timeout`: `--connect-timeout` around
  `KrpcClient::connect`, `--max-time` around connect+call. `KrpcClient` exposes no timeout
  knobs, so wrapping is the clean, krpc-unmodified path. Elapse → `CliError::Timeout` (exit 5).
- **`violations`** stays omitted (never fabricated) pending AGENT-002's server-side versioned
  Status-detail schema (design §2.2 / OQ1) — the record shape is ready; extraction is not
  wired because there is no local source format in this checkout.

## Not done (by design / external gating)

- P1 backlog (`-s/-S`, `--fail-with-body`, `--include`/`meta`, https introspection, retry,
  `rpcurl mcp`, `-o`, `-K`) — untouched, per §3.2.
- No GitHub Release, no tag, no native-smoke unpin — maintainer-owned (§3.3). native-smoke
  will need the pin-audit-unpin pass because 1.1.0 changes the default output, drops
  `-f`/`-c`/`--json`, and reshapes the machine error record.

---

## impl r1 — review fixes (I1–I6)

Closes the six blocking findings from the implementation review. Same implementation
commit (amended); design commit `ba555de` still intact; design-doc amendments below are
in this commit (doc↔code lockstep).

### I1 — help/version obey the resolved mode
`main.rs::emit_help`/`emit_version` (called from `handle_parse_error`): machine mode emits
a single JSON object on **stdout** — `--version` → `{"name":"rpcurl","version":"<v>"}`,
`--help` → `{"help":"<rendered help>"}`; human mode prints clap's text; exit 0.
Design amended (§2.1 help/version bullet). Tests: `version_json_mode_is_structured`,
`help_json_mode_is_structured`, `version_human_mode_is_plain_text`.

### I2 — `-v` goes through the JSONL diagnostics channel
New `output.rs::render_debug` → `{"type":"debug","label":…,"message":…}` in machine mode,
`[label]` blocks in human mode; all four verbose points in `main.rs::invoke` route through
it. Header dumps use `main.rs::dump_metadata`, which **redacts `authorization`/`cookie`**
(no bearer/cookie leak). Design de-contradicted: §2.3 now defines a `type:"debug"` record
and §1.5/§2.1 are consistent (removed the "outside JSONL" clause; README updated). Tests:
`verbose_machine_mode_is_pure_jsonl` (every stderr line parses; order debug…warning…error),
`verbose_does_not_leak_bearer_token`, unit `output::debug_*`.

### I3 — bare `-c` is a hard error pointing at `-b`
Removed the hidden clap `cookie_jar` field; `main.rs` now pre-scans raw argv with
`args::is_cookie_jar_flag` (covers `-c`, `-cVALUE`, `--cookie-jar`, `--cookie-jar=…`)
**before** clap value handling and renders `args::COOKIE_JAR_MSG` (names `-b`). Exit 2.
Tests: `bare_c_is_hard_error_pointing_at_b`, `cookie_jar_c_is_hard_error_pointing_at_b`,
unit `cookie_jar_flag_detected_in_all_forms`. The earlier documented exception is gone.

### I4 — did-you-mean scoped honestly; invoke self-corrects via `cli.hint`
did-you-mean (edit-distance) is **introspection-only** (`discover::find_method` — it has
the catalog). The invoke path has no local method catalog, so it cannot do edit-distance;
instead `main.rs::with_hint` attaches `cli.hint`: `INVALID_ARGUMENT` → `rpcurl schema …`,
**`UNIMPLEMENTED` (method not found) → `rpcurl discover …`** (added this round);
`split_url` returns explicit format guidance. Design amended (§2.5, §3.1 item 9, §5 row 7).
Tests: `schema_typo_suggests_nearest_method`, unit `hint_targets_invalid_argument_and_unimplemented`.

### I5 — bounded `-d @-`/@file with redacted diagnostics
`args::read_capped` caps file/stdin reads at **8 MiB** (`MAX_BODY_BYTES`) via a
`Read::take` — oversized input → usage error (summarized, never copied); non-UTF-8 → usage
error carrying no raw bytes. Parse errors now report **byte length + parser location only**
(`invalid request json (N bytes): <serde loc>`), never the raw body. Design amended
(§1.1 row, §2.4 buffering/limits). Tests: `oversized_stdin_is_capped_usage_error`,
`non_utf8_stdin_is_usage_error_without_raw_bytes`.

### I6 — restored routing test + completed migration list + edge coverage
- **`bad_url_is_usage_error_exit_2`** — restored (and strengthened) the stream assertion:
  now asserts empty stdout **and** a JSONL error record on stderr (`type:"error"`,
  `cli.kind:"usage"`), exit 2. *Migration honesty:* the r1 rewrite had dropped the
  original `!stderr.is_empty()` assertion; that was an unrecorded weakening — corrected and
  documented here.
- **TTY-split** coverage: `success_puts_data_on_stdout_and_nothing_on_stderr` (clean
  success = payload on stdout, empty stderr).
- Full edge set added this round: verbose JSONL + no-leak, bare `-c`, help/version json +
  human, oversized/non-UTF-8 stdin, restored malformed-URL routing.

### Design-doc amendments this round (in this commit)
§2.1 (help/version machine shape), §2.3 (`type:"debug"` record + verbose framing; removed
the "outside JSONL" contradiction), §1.1 & §2.4 (8 MiB cap + parse-error redaction), §2.5
/§3.1 item 9 / §5 row 7 (did-you-mean = introspection; invoke `cli.hint` incl. UNIMPLEMENTED
→ discover). README updated to match (verbose, help/version).

### Gates (impl r1)
`cargo test -p rpcurl` → **69 passed** (36 unit + 33 integration); `cargo clippy
--all-targets -p rpcurl -- -D warnings` → **0 warnings**; `cargo build -r -p rpcurl` → OK.

---

## impl r2 — I2 residual (hostname leak)

The verbose `type:"debug"` request-headers dump printed the default `c-id` value
`r-<hostname>`, leaking the invoking machine's hostname into machine-captured logs (repo
rule: internal hostnames never enter logs).

**Fix:** `main.rs::dump_metadata` now redacts a set `REDACTED_META =
{authorization, cookie, c-id, c-meta}`. Decision on `c-meta`: **redact** — it is
caller-supplied and can carry identifying values; the verbose dump is a debug aid, not a
wire audit, so redaction costs nothing and removes a leakage class. **Wire headers are
unchanged** (`fill_headers` still sends the real `c-id`/`c-meta`); only the `-v` render is
affected.

**Tests:** `verbose_does_not_leak_hostname` (new) — runs `-v --format json`, resolves the
real hostname via a `gethostname` dev-dependency, and asserts the hostname string is absent
from **both** stdout and stderr while `c-id: <redacted>` is present. `verbose_does_not_leak_bearer_token`
kept. Design amended (§2.3 debug-record redaction set) and README updated.

### Gates (impl r2)
`cargo test -p rpcurl` → **70 passed** (36 unit + 34 integration); `cargo clippy
--all-targets -p rpcurl -- -D warnings` → **0 warnings**; `cargo build -r -p rpcurl` → OK.

---

## impl r3 — maintainer design change: default JSON, `--human` opt-in

Maintainer overrode the `--format auto` (TTY-gated) default. New output contract:
**machine JSON unconditionally by default** (no TTY detection for payload or
diagnostics — deterministic in a terminal or a pipe); **`--human`** (boolean) is the
**only** switch into decorated output; `--format`/`auto`/`IsTerminal` removed entirely.
help/version follow the same rule (JSON default, decorated under `--human`).

**Code:**
- `args.rs`: removed the `Format` enum; replaced `--format` with a global boolean
  `--human`.
- `main.rs`: removed `resolve_mode`, `format_from_argv`, `parse_format`, and the
  `std::io::IsTerminal` import/usage. Mode is now `if cli.human { Human } else { Json }`;
  the pre-parse path sniffs `--human` from argv (`argv_mode`). All stream-routing
  invariants and the redaction set (`REDACTED_META`) are unchanged.
- Verified: `grep IsTerminal|Format|resolve_mode|format_from_argv src/` → none.

**Tests migrated (`tests/cli.rs` rewritten for the new contract):** every `--format json`
argument dropped (JSON is the default); `--format human` → `--human`; the now-redundant
auto-vs-json duplicate tests merged. Net: default-mode tests assert JSON in all harness
contexts (simpler — no TTY dependence); `--human` tests assert emoji/decorated output.
Kept green: envelope, routing, bearer+hostname redaction, timeout(5), did-you-mean,
`-c`/bare-`-c`, `@-`/oversized/non-UTF-8, help/version (default JSON + `--human` text),
verbose JSONL. Note: `--human` before a subcommand collides with clap's top-level `url`
positional (`args_conflicts_with_subcommands`), so global flags go **after** the
subcommand (`rpcurl discover <base> --human`) — matches how `--json` was used pre-change.

**Docs:** DESIGN §1.6, §2.1 (whole output-mode block), §2.3 (machine-mode line + mode
table), §2.4, §2.5, §3.1 item 1, §3.3, §5 item 1 rewritten to the default-JSON + `--human`
contract; README output section + flags table updated.

**Smoke:** default (no flag) → JSON envelope; `--human` → `❌`/🍀; `--version` → JSON,
`--human --version` → `rpcurl 1.1.0`; no `IsTerminal` in the binary source.

### Gates (impl r3)
`cargo test -p rpcurl` → **67 passed** (36 unit + 31 integration); `cargo clippy
--all-targets -p rpcurl -- -D warnings` → **0 warnings**; `cargo build -r -p rpcurl` → OK.
