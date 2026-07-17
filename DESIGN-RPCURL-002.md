# DESIGN-RPCURL-002 — rpcurl redesign: agent-native CLI (curl as reference)

> Status: DESIGN ONLY (no code in this train). Owner: omp. Reviewer: codex.
> Branch `design/rpcurl-002` off `fd4d47d` (RPCURL-001 / PR #1).
> Revision r5: **maintainer redirect** — agent-native first; curl is a *reference*,
> not a target; **compatibility is a non-constraint**; everything ships in **one clean
> release, 1.1.0** (all prior 2.0.0→3.0.0 / SemVer-major / deprecation-window machinery
> is deleted). Cost/benefit (性价比) evaluated per feature. Commits LOCAL, no push.

## 0. Mandate & basis

**Goal (maintainer ruling, verbatim intent):** redesign rpcurl to be **agent-native**
— machine-parseable by default where safe, self-describing, self-correcting. **curl is a
familiarity reference only**: adopt a curl convention *only when it transfers knowledge
at zero design cost*; never chase curl consistency for its own sake.

**Ruling consequences baked into this design:**
- **Compatibility is a non-constraint.** Few users; a clean re-architecture is
  authorized. No aliases, no deprecation windows, no dual-accept grammars.
- **One release: `1.1.0`.** Every item marked 1.1.0 in §1/§3.1 ships in this release. (Normal SemVer would call a
  default-output change "major"; the maintainer waived compat, so the version tag is just
  the next release, not a migration event.) The only downstream coordination is our own
  CI consumer — one paragraph in §3, not a phase plan.
- **性价比 per feature.** Every row in §1 and §5 carries **agent value** and **effort**;
  pure curl-mimicry with low agent value is cut or backlogged, not shipped.

### Verification basis

- **Current rpcurl behavior**: every claim cites `crates/rpcurl/<file>:<line>` from this
  checkout (`fd4d47d` + worktree). Nothing asserted from memory.
- **curl behavior**: `[curl:8.7.1]` = verified against the curl 8.7.1 manual (codex r1);
  `[curl:verify]` = confirm against `man curl` before coding. curl is cited to gauge
  *knowledge transfer*, never as a compliance target.
- **Cross-train dependency**: AGENT-002 (MCP error envelope) is a *separate* worktree
  (`wt-agent002`); its goal doc is **not in this checkout**. References to its envelope /
  server-side Status-detail encoding are dependencies on an external, unmerged train,
  flagged where they land (§2.2, §4).

### Wire reality (do not confuse with curl's HTTP model)

rpcurl has **two** transports; neither is JSON-RPC:
- **Invoke** (default path): builds `InputProto { json }` — a protobuf message whose
  `json` field carries the request body string — and sends it via
  `KrpcClient::connect(host).call(method, req)` over **KRPC gRPC** (`args.rs:129-135`,
  `main.rs:116-125`). The response `OutputProto.out` is one of `Out::Json(String)`,
  `Out::Bytes(Vec<u8>)`, `Out::Error(String)`, or `None` (`output.rs:26-37,58-64`) — it
  is **not "always JSON"**.
- **Introspection**: plain HTTP `GET {origin}/agent/discover` (`discover.rs:420-447`).

curl's `--json` is an HTTP request shortcut (`--data` + `Content-Type`/`Accept` headers);
on the gRPC invoke path there are **no HTTP request headers to set**, so there is no
wire-level curl-`--json` equivalent (§1, `--json` row). Respects AGENTS.md NS-2: rpcurl
follows the wire, never forks it.

### Verified current surface (grounding)

Default: `rpcurl <url> [flags]` → gRPC call. Subcommands `discover`/`schema`/`example` →
HTTP `/agent/discover`.

| Flag | Short | Long | Env | Effect | Source |
|---|---|---|---|---|---|
| url (positional) | — | — | — | `scheme://host/app/Svc/method`; needs `://` + a `/` | `args.rs:56`, `split_url` `:95-109` |
| data | `-d` | `--data` | — | JSON string; **beats `-f`**; validated | `args.rs:59-60`, `read_data` `:113-125` |
| file | `-f` | `--file` | — | `fs::read_to_string` | `args.rs:63-64`,`:116-118` |
| token | `-t` | `--token` | `KRPC_TOKEN` | → `authorization: Bearer <t>` | `args.rs:67-68`,`:179-186` |
| cookie | `-c` | `--cookie` | `KRPC_COOKIE` | → `cookie:` header (**sends**) | `args.rs:71-72`,`:187-193` |
| client id | `-i` | `--c-id` | `KRPC_CID` | → `c-id`; default `r-<hostname>` | `args.rs:75-76`,`:161-170` |
| client meta | `-m` | `--c-meta` | `KRPC_CMETA` | → `c-meta` | `args.rs:79-80`,`:172-178` |
| header | `-H` | `--header` | — | repeatable; **`k=v` split on first `=`**; malformed→warn | `args.rs:83-84`,`:147`,`:156`, `main.rs:108-111` |
| machine out | — | `--json` | — | global; pure JSON; errors as JSON on stderr | `args.rs:22-23`, `main.rs:27` |
| verbose | `-v` | `--verbose` | — | global; diagnostics → **stderr** | `args.rs:26-27`, `main.rs:99-133` |

Data precedence `-d`>`-f`>`null` (`args.rs:113-121`). Routing: success→stdout,
error→stderr, always via injected `Write` (`output.rs:1-5`). **No TTY detection anywhere**
— mode is purely the `--json` flag (`main.rs:27`); a non-TTY stdout still gets human
today. Exit codes `0/1/2/3/4` (`error.rs:53-60`). Machine error today
`{"error":{"kind","msg","code?}}` (`output.rs:82-93`). Version `1.0.0` (`Cargo.toml:3`),
GitHub-Release musl binaries.

---

## 1. Flag/behavior matrix — the agent-value lens

Verdict ∈ **1.1.0** (ship now) · **P1** (backlogged) · **not planned** (cut). Each row
carries **agent value** and **effort** (H/M/L). curl is shown only to note whether a
convention transfers for free. Timing detail: everything marked 1.1.0 ships together;
there is no migration window (compat waived).

### 1.1 Data input

| rpcurl today | curl ref | Verdict · value/effort | Resolution |
|---|---|---|---|
| `-d '<json>'` (`args.rs:59-60`) | `-d` sends arg verbatim `[curl:8.7.1]` | **1.1.0** · H/L | Keep `-d`. Raw JSON string; `@`-prefix = file, `@-` = stdin (below). curl-transfer is free. |
| `-f <path>` (`args.rs:63-64`) | curl reads files via `-d @file` `[curl:8.7.1]` | **1.1.0** (replace) · H/S | Fold file input into `-d @file`; **drop `-f`/`--file`**. One obvious way to pass a body; agents already think `@file`. `-f` short is freed (left unassigned). |
| (no stdin) | `-d @-` = stdin `[curl:8.7.1]` | **1.1.0** · H/S | `-d @-` reads stdin. Agents pipe generated bodies. File/stdin reads are **capped at 8 MiB** (bounds memory on a hostile pipe); a JSON parse error reports byte length + parser location only — **never the raw body** — so a token/PII payload with a trailing syntax error is not echoed to diagnostics/CI logs. Non-UTF-8 input → usage error carrying no raw bytes. |
| `-d` beats `-f` silently (`args.rs:114-121`) | curl joins multiple `-d` `[curl:8.7.1]` | **1.1.0** · M/S | **Multiple body sources = usage error (exit 2)**, not silent last-wins. curl's `&`-join is form semantics, meaningless for one JSON doc; erroring is the agent-honest choice. |

### 1.2 Headers

| rpcurl today | curl ref | Verdict · value/effort | Resolution |
|---|---|---|---|
| `-H 'k=v'` split on `=` (`args.rs:147`) | `-H 'Name: value'` colon `[curl:8.7.1]` | **1.1.0** · M/S | Accept `Name: value` (curl-transfer free) **and** keep `k=v` — both parse unambiguously (colon first, else equals), no window needed. Header names are **lowercased** (tonic metadata keys must be ascii-lowercase; also lets an agent paste a curl `Content-Type:` header verbatim). No `W_HEADER_SEPARATOR` warning — both forms are first-class (compat waived). |
| malformed `-H`→warn, ignored (`args.rs:156`, `main.rs:108-111`) | curl handles specially `[curl:verify]` | **1.1.0** · M/S | Keep warn-not-fail, but emit through the **structured diagnostics channel** (§2.3, `W_HEADER_MALFORMED`), not a bare stderr line. Agents need to detect the warning programmatically. |

### 1.3 Cookies — the `-c` trap (agent-native win regardless of curl)

| rpcurl today | curl ref | Verdict · value/effort | Resolution |
|---|---|---|---|
| `-c/--cookie` = **send** (`args.rs:71-72`,`:187-193`) | `-b/--cookie`=send; `-c/--cookie-jar`=write jar `[curl:8.7.1]` | **1.1.0** · H/S | rpcurl's `-c` sends a cookie, but a curl user's `-c` **saves a jar** — an inverted, dangerous collision. **Fix: cookie-send = `-b`/`--cookie`; `-c` becomes a hard usage error (exit 2)** pointing to `-b`. This is an agent-native correctness win (kills a silent-wrong path), not curl-mimicry. rpcurl has **no cookie jar**, so curl's real `-c` (dump in-memory store, create/truncate even when empty, non-fatal write `[curl:8.7.1]`) has nothing to implement — `--cookie-jar` is **not planned** (N/A). |

### 1.4 Auth

| rpcurl today | curl ref | Verdict · value/effort | Resolution |
|---|---|---|---|
| `-t/--token` → Bearer (`args.rs:67`,`:179-186`) | `--oauth2-bearer` = Bearer; `-u`=Basic `[curl:8.7.1]` | **1.1.0** · M/S | Keep `-t`/`--token`/`KRPC_TOKEN` (the working agent path). Add `--oauth2-bearer` **only** as a zero-cost familiar alias (same code path). Basic-auth `-u` = not a KRPC concept, **not planned**. |

### 1.5 Output routing, silence, file

| rpcurl today | curl ref | Verdict · value/effort | Resolution |
|---|---|---|---|
| success→stdout, error→stderr (`output.rs:1-5`) | same `[curl:8.7.1]` | **1.1.0** · H/— | Load-bearing agent contract; already correct. Exact stream table in §2.1. |
| `-v` → stderr (`main.rs:99-133`) | `-v` → stderr `[curl:8.7.1]` | **1.1.0** · M/— | Keep; verbose lines become structured diagnostics (§2.3). |
| (no `-s`/`-S`) | `-s` silent, `-S` show-error `[curl:8.7.1]` | **P1** · M/S | `-s` (suppress diagnostics) / `-S` (re-show errors under `-s`). Real agent value (quiet batch runs) but not blocking the flagship; cheap when done. |
| (no `-o`) | `-o <file>` `[curl:8.7.1]` | **P1** · L/S | Shell redirection already covers this; low marginal agent value. Backlog. |

### 1.6 Output mode — machine-JSON default + `--human`

| rpcurl today | curl ref | Verdict · value/effort | Resolution |
|---|---|---|---|
| `--json` output flag; **no TTY detect** (`main.rs:27`) | curl `--json` = HTTP request shortcut (N/A on gRPC) | **1.1.0** · H/M | **Default = machine JSON, unconditionally** (bare JSON on stdout + JSONL stderr; **no TTY detection**, terminal or pipe alike). **`--human`** (boolean) is the only switch into decorated output. `--format`/`auto`/`--json` are all dropped. Rationale: deterministic mode-independent output beats TTY-sniffing for an agent-first tool; humans opt in explicitly. |

### 1.7 Fail semantics & error body

| rpcurl today | curl ref | Verdict · value/effort | Resolution |
|---|---|---|---|
| remote error → exit **1** (`main.rs:120-125,138-140`) | curl exits 0 on HTTP≥400 unless `--fail` `[curl:8.7.1]` | **1.1.0** · H/— | Keep **fail-by-default** — the agent-native answer (non-zero on server error, no flag needed). curl's explicit `--fail` no-op affirmation adds nothing → **not planned**. `--no-fail` opt-out → **not planned** (low value). |
| (no error body on stdout) | `--fail-with-body` `[curl:8.7.1]` | **P1** · M/S | `--fail-with-body` — HTTP-face-only (single definition, §2.1). Useful for reading an introspection/MCP error body; not needed for the gRPC invoke path (no body). Backlog. |

### 1.8 Timeouts, retry, write-out

| rpcurl today | curl ref | Verdict · value/effort | Resolution |
|---|---|---|---|
| (none) | `--connect-timeout`, `-m/--max-time` `[curl:8.7.1]` | **1.1.0** · H/M | Add `--max-time <s>` (overall) + `--connect-timeout <s>` (per attempt). Agents need bounded waits. Compat waived → **`-m` is reassigned to `--max-time`**; c-meta moves to `--client-meta` (long-only, §1.9). curl-transfer of `-m` is a bonus, not the reason. |
| (none) | `--retry` `[curl:8.7.1]` | **P1** · M/L | Not trivially cheap and idempotency-unsafe for invoke. If built: introspection GETs retry by default; invoke only under explicit `--retry-unsafe` with full-body buffering + `--max-time` as the overall budget (§2.4). Backlog. |
| (none) | `-w/--write-out %{...}` `[curl:8.7.1]` | **not planned** · L/M | Agents already get the result envelope on stdout, the gRPC code in the error record, and timing is cheap to measure externally. curl's `%{json}` (JSON of write-out vars) is ergonomic sugar with low agent value. Cut. |

### 1.9 Tracking metadata & config

| rpcurl today | curl ref | Verdict · value/effort | Resolution |
|---|---|---|---|
| `-i/--c-id` (`args.rs:75`), `-m/--c-meta` (`args.rs:79`) | — (rpcurl-specific) | **1.1.0** · M/S | Rename to **`--client-id` / `--client-meta`** (long-only; keep `KRPC_CID`/`KRPC_CMETA`). Frees `-i` and `-m` for agent-native reuse (`-m`=max-time §1.8; `-i` left for a future `--include`, P1). Clean re-arch, no aliases. |
| (no config) | `-K/--config`, `~/.curlrc` `[curl:8.7.1]` | **P1** · L/M | `~/.rpcurlrc` for default host/token/headers. `KRPC_*` env already covers the common agent case → low priority. |

### 1.10 Introspection response metadata

| rpcurl today | curl ref | Verdict · value/effort | Resolution |
|---|---|---|---|
| (no include) | `-i/--include` prepends headers `[curl:8.7.1]` | **P1** · M/M | `--include` → response metadata as the structured `meta` side-channel (schema §2.3), never raw-prepended (would corrupt bare-JSON stdout). Moderate agent value (trace ids); backlog. `-i` short reserved for it. |

### 1.11 Exit codes — already the agent-native answer

curl has ~90 sparse codes `[curl:8.7.1]`; rpcurl keeps a **small, documented set** — the
right call for agents branching on `$?`, and unchanged by this redirect. curl's exit `1`
(unsupported protocol) even *conflicts* with rpcurl's exit `1` (remote app error), so
number-mimicry is impossible anyway. `--curl-exit-codes` compat mode → **not planned**.

| code | kind | meaning |
|---|---|---|
| 0 | — | success |
| 1 | remote | server reachable, returned app/gRPC error (fail-by-default) |
| 2 | usage | bad CLI input / parse error |
| 3 | connect | could not resolve/connect |
| 4 | protocol | reached, non-2xx / unparseable (HTTP faces) |
| 5 | timeout | connect/max-time exceeded (**new**, 1.1.0) |

**Value/effort: H/S** — the taxonomy exists (`error.rs:53-60`); only code `5` is added.

---

## 2. Agent-native spec

### 2.1 Streams & the default-JSON contract (`--human` opts in)

**Stream invariant (unchanged from today's `output.rs`, hardened):**
- **stdout carries success *payload* only.** On error, stdout receives bytes **only**
  under `--fail-with-body` (P1) with a server body (§1.7). Sole exception.
- **stderr carries diagnostics** (verbose, warnings, terminal error record) — never
  success payload.
- **No TTY detection.** The mode is fixed by the `--human` flag alone (absent = machine
  JSON), never by whether stdout is a terminal — deterministic in a terminal or a pipe.

**Per-outcome contract:**

| Outcome | stdout | stderr | exit |
|---|---|---|---|
| success | payload envelope (or file if `-o`, P1) | verbose/warnings | 0 |
| error, default | *(empty)* | terminal error record (§2.2) | non-zero |
| error, `--fail-with-body` (P1) | raw HTTP body bytes (HTTP faces only; gRPC invoke: empty) | terminal error record | non-zero |
| error, `-s` / `-sS` (P1) | as above | suppressed / error record | non-zero |

`-o` (P1) redirects the same stdout bytes. Mode (`--human`, else default JSON) controls
success-payload rendering and stderr diagnostics; `--fail-with-body` stdout remains
verbatim. No combination puts an error *record* on stdout or success payload on stderr.

**`--fail-with-body`, one definition (P1):** *body* = the **raw HTTP response-body
bytes** of a face; exists **only on HTTP faces** (introspection today; future MCP/HTTP).
A gRPC error is a `Status`/`Out::Error` *message* (`main.rs:120-125,152-159`), **not a
body** → the flag is N/A/no-op on invoke. On HTTP faces it writes exact raw bytes,
**exempt from JSON-output validity** (the body may not be JSON; mode does not reshape it).

**Default = machine JSON, unconditionally (flagship, 1.1.0).** No TTY detection for
payload or diagnostics: a terminal and a pipe both get bare JSON on stdout + JSONL
diagnostics on stderr. Deterministic everywhere — the agent-native default.

- **`--human`** (boolean, the only switch) opts into decorated output: emoji/pretty
  payload on stdout + decorated (⚠️/❌/`[label]`) diagnostics on stderr.
- There is **no `--format` enum, no `auto`, no `IsTerminal` mode logic** — all removed.
- **`--help` / `--version` obey the same rule**: default emits a single JSON object on
  stdout (`--version` → `{"name":"rpcurl","version":"<v>"}`; `--help` →
  `{"help":"<rendered help text>"}`); `--human` prints clap's usual text. Exit 0.

Rationale: deterministic, mode-independent output beats TTY-sniffing for an agent-first
tool — an agent gets identical machine output whether piped or on a terminal, and humans
opt in explicitly. This supersedes the earlier `--format auto` TTY-gated proposal (an
explicit maintainer ruling for this clean release).

### 2.2 Machine error record — the core contract (AGENT-002 aligned)

The machine error emits, at the **top level**, exactly the AGENT-002 triplet
`{code, message, violations?}` — byte-compatible in keys/value-domains/semantics — with
CLI-only fields isolated under a namespaced sibling `cli`, and a diagnostics-framing
`type` (§2.3). A consumer that knows AGENT-002 reads `code`/`message`/`violations`
unchanged and MAY ignore `type`/`cli`.

Terminal error record (machine mode, one JSONL line on stderr):

```json
{"type":"error","code":"INVALID_ARGUMENT","message":"name: must not be blank","violations":[{"field":"name","constraint":"NotBlank","rejected":""}],"cli":{"kind":"remote","exit":1}}
```

- **`code`** — **string, monotype**. Remote: the `google.rpc.Code` **name** from the
  tonic `Status` (`main.rs:122` keeps the numeric code; the builder maps `3 →
  "INVALID_ARGUMENT"`). CLI-local: `"USAGE"`/`"CONNECT"`/`"PROTOCOL"`/`"TIMEOUT"`. Numeric
  gRPC code, if needed, lives in `cli.grpcCode`.
- **`message`** — string (replaces today's `msg`, `output.rs:87`).
- **`violations`** — optional `[{field, constraint, rejected?}]`, present **only** when
  the server supplies structured field violations. **Depends on AGENT-002** (external):
  until its server publishes a **versioned Status-detail encoding**, rpcurl has no local
  source format and **omits `violations`** (graceful degradation, never fabricated). §4.
- **`cli`** — `{kind, exit, grpcCode?}`; `kind` ∈ `usage|connect|remote|protocol|timeout`.
- **`type`** — `"error"`. `type` and `cli` are the **only** framing keys beside the triplet.

The old `{"error":{"kind","msg","code?}}` shape is simply gone in 1.1.0 (compat waived —
no dual emission). Success envelope `code` stays a **numeric** wire code (`output.rs:24-38`),
a distinct namespace from the error record's string `code` — documented, not merged.

**Value/effort: H/M** — the flagship data contract; every agent error path depends on it.

### 2.3 Diagnostics & warnings contract (JSONL)

Warnings (malformed `-H`, tolerated separator) and the terminal error share **one framed
stderr stream**:

- **Machine mode** (the default; `--human` absent): **JSONL**, exactly one JSON object per
  stderr line, each with `type` ∈ `"warning"|"error"|"debug"`, so a consumer distinguishes
  a warning or a `-v` debug line from the terminal error object.
- **Warning record**: `{"type":"warning","code":"<STABLE_CODE>","message":"..."}`. Stable
  codes: `W_HEADER_MALFORMED` (the only P0 warning today), … (crate registry; additive).
- **Error record**: the §2.2 object with `type:"error"`.
- **Debug record** (`-v` only): `{"type":"debug","label":"…","message":"…"}` — verbose
  dumps are framed, not raw prose, so `-v` never corrupts the JSONL stream. Sensitive
  values for `authorization`, `cookie`, `c-id`, and `c-meta` are **redacted** in these
  dumps — `c-id` defaults to `r-<hostname>` (an internal hostname; repo rule: never into
  logs) and `c-meta` may carry caller-identifying values. The wire headers are unchanged.
- **Ordering**: processing order; terminal error is the **last** record; ≤1 `error` per run.
- **Success-with-warnings**: warnings on stderr, payload on stdout, **exit 0**.
- **Versioning**: diagnostics schema **v1**; a framing break bumps the CLI version and is
  noted in release notes. Human mode uses decorated lines (⚠️/❌), not part of the machine
  contract. **Verbose (`-v`) emits framed `type:"debug"` records in machine mode** and
  decorated `[label]` blocks in human mode — never raw prose on a machine stderr stream.
- **`-s`/`-S`** (P1): `-s` suppresses all diagnostics; `-S` restores the error record.

**Diagnostics & payload by output mode** (single source of truth):

| mode | stdout payload | stderr diagnostics | `meta` (`--include`, P1) |
|---|---|---|---|
| default (machine) | bare JSON envelope | JSONL (`type`-framed) | `meta` field on the stdout envelope |
| `--human` | decorated (emoji/pretty) | decorated (⚠️/❌) | decorated `[response meta]` on stderr |

**Response-metadata side-channel (`meta`) — schema (for `--include`, P1):** a JSON object
where **every key maps to an array of values** (always arrays; order preserves wire
repetition):

```json
{"meta":{"content-type":["application/grpc"],"x-trace":["a","b"],"token-bin":["aGVsbG8="]}}
```

- ASCII keys → arrays of UTF-8 strings. **`-bin` keys** (gRPC binary metadata) →
  **base64** string arrays; the `-bin` suffix **is** the encoding marker (no `encoding`
  field). Placement: default (machine) → `meta` on the stdout envelope; `--human` →
  decorated stderr block, never mixed into stdout.

**Value/effort: H/M** — prerequisite for any structured warning; ships in 1.1.0 even
though `--include` (its `meta` consumer) is P1, because the error framing needs it.

### 2.4 Introspection surface (already strong — keep)

`discover <base>` / `schema <base> <method>` / `example <base> <method>` over HTTP
`/agent/discover` (`args.rs:30-51`, `discover.rs:564-589`), honoring the mode (`--human`). Keep all
three verbs unchanged (curl has no analog; nothing to rename). `discover` lists
`path`/`arg`/`res`/`doc` (`discover.rs:453-484`); `schema` returns `{method,doc,input,
output}` with validation folded in (`discover.rs:514-522`); `example` a skeleton
(`discover.rs:544-558`).

- **Grammar note**: introspection takes a **base** url; invoke takes a **full method**
  url — document prominently (a common agent stumble).
- **Body buffering / limits**: `-d @-` stdin is drained once, capped at **8 MiB**
  (`MAX_BODY_BYTES`); oversized input → usage error (exit 2), summarized never copied.
  Parse errors report length + location, not the body (redaction).
- **Limitation (A-3)**: introspection is **http-only** (`discover.rs:425-429`); TLS
  servers can't be discovered. https support = **P1** (value M, effort M).

**Value/effort: —/none** — exists at `fd4d47d`; no P0 work.

### 2.5 Self-correction loop (agent walkthrough)

```
1. $ rpcurl discover http://host/quickstart        # default → machine JSON (no flag)
   → {"services":[{"name":"Hello","methods":[
        {"path":"Hello/hello","arg":"HelloRequest","res":"HelloReply","doc":"..."}]}]}
2. $ rpcurl schema http://host/quickstart Hello/hello
   → {"method":"Hello/hello","input":{"type":"object",
        "properties":{"name":{"type":"string","minLength":1},"age":{"type":"integer"}},
        "required":["name"]},"output":{...}}
3. $ rpcurl example http://host/quickstart Hello/hello
   → {"name":"string","age":0}
4. $ rpcurl http://host/quickstart/Hello/hello -d '{"age":28}'   # piped → machine JSON
   stderr: {"type":"error","code":"INVALID_ARGUMENT","message":"name: must not be blank",
            "violations":[{"field":"name","constraint":"NotBlank"}],
            "cli":{"kind":"remote","exit":1}}       # violations only if AGENT-002 merged
   exit 1
5. Agent reads violations[0].field (or parses message pre-AGENT-002), retries:
   $ rpcurl http://host/quickstart/Hello/hello -d '{"name":"KRPC","age":28}'
   stdout: {"code":0,"data":{"greeting":"Hi KRPC"}}   exit 0
```

The default machine-JSON mode means **no output flag anywhere in this loop** — you just
run it. Two local wins ship in 1.1.0 (no server change, high agent value):
- **did-you-mean (introspection)**: `find_method` failures (`discover.rs`) suggest the
  nearest known `Service/method` path (edit distance ≤2) — the discover catalog is in
  hand, so this is a local suggestion. **Value/effort: H/M.**
- **error → self-correction hint (invoke)**: the invoke path has **no local method
  catalog** (no discover fetched), so it can't do edit-distance did-you-mean; instead a
  `remote` error carries a `cli.hint` pointing at the right recovery command —
  `INVALID_ARGUMENT` → `rpcurl schema <host> <method>`, `UNIMPLEMENTED` (method not
  found) → `rpcurl discover <host>`. `split_url` structural failures return explicit
  format guidance. **Value/effort: H/S.**

### 2.6 MCP verb (P1, gated)

`rpcurl mcp list/call` to drive the server's `/mcp` face for parity testing. **Value: M**
(makes AGENT-002's field findings CI-testable; `/agent/invoke` vs `tools/call` parity).
**Effort: L** (new subcommand, HTTP client reuse, error-record reuse). **Gated**: the
`/mcp` protocol version, transport, and `initialize` sequence are AGENT-001/002
deliverables **not in this checkout** — not implementable until pinned (§4). Client-only,
zero server surface. Backlog.

---

## 3. Delivery

### 3.1 One release: 1.1.0 (P0 scope)

Every item marked 1.1.0 (below) ships together in this release; no migration window (compat waived):

1. Default machine JSON + **`--human`** opt-in (flagship; no TTY detection) — §1.6/§2.1
2. Machine error record `{code,message,violations?}`+`type`+`cli` (`violations` degrades) — §2.2
3. Diagnostics JSONL contract (warnings + error framing + `meta` schema) — §2.3
4. `-c` hard-error + `-b` cookie-send — §1.3
5. `-d @file` / `-d @-` / drop `-f` / one-body-error — §1.1
6. exit set `0–5` (add `5`=timeout) — §1.11
7. `--max-time` + `--connect-timeout` (`-m` reassigned) — §1.8
8. `--client-id` / `--client-meta` rename (`-i`/`-m` freed), `-H` colon+equals, `--oauth2-bearer` alias — §1.2/§1.4/§1.9
9. did-you-mean (introspection) + invoke `cli.hint` (schema/discover pointers) — §2.5

### 3.2 P1 backlog (each value/effort)

| Item | Value | Effort | Note |
|---|---|---|---|
| `-s` / `-S` silent/show-error | M | S | quiet batch runs |
| `--fail-with-body` (HTTP-face-only) | M | S | read HTTP/MCP error bodies |
| `--include` + `meta` side-channel | M | M | trace ids; `-i` reserved |
| https introspection | M | M | fixes A-3; unblocks TLS servers |
| retry (guarded) | M | L | GET-only default; `--retry-unsafe` for invoke |
| `rpcurl mcp list/call` | M | L | gated on pinned MCP contract (§4) |
| `-o <file>` output | L | S | shell redirection covers it |
| `-K` / `~/.rpcurlrc` config | L | M | env already covers common case |

**Not planned** (curl-mimicry, low agent value): `--fail` no-op, `--no-fail`,
`-w/--write-out`/`%{json}`, `--curl-exit-codes`, cookie-jar (`--cookie-jar`), basic-auth `-u`.

### 3.3 Native-smoke coordination (our only downstream)

The external `krpc` native-smoke CI pulls the latest linux-musl `rpcurl` (AGENTS.md);
it is our consumer, not present in this checkout. Because 1.1.0 changes the default output
(now machine JSON by default), the drop of `-f`/`-c`/`--json`/`--format`, and the error-record shape, **pin native-smoke to
the `1.0.0` binary, audit its rpcurl invocations against the 1.1.0 surface (flags, streams,
exit codes), update them, then unpin.** One coordinated bump — no phased plan.

---

## 4. Open questions (post-ruling)

The maintainer's ruling resolved the prior OQs on clean-break, version scheme, exit
numbers, and the output-mode default (all settled inline above). What remains genuinely
open:

1. **`violations[]` sourcing / AGENT-002 dependency.** rpcurl's error record references
   AGENT-002's `{code,message,violations?}` and its server-side Status-detail encoding,
   which is **external and unmerged**. Recommendation + default: **ship 1.1.0 with the
   record shape and `violations` omitted** (graceful degradation), wire extraction once
   AGENT-002 publishes a versioned detail schema. Only the `violations` sub-feature waits.
2. **MCP verb contract (§2.6).** `rpcurl mcp list/call` needs the pinned MCP protocol
   version, transport, and `initialize` sequence from AGENT-001/002 (not in this
   checkout). Recommendation + default: **P1, gated**; implement once the contract is
   pinned. Mark experimental when built.

---

## 5. 性价比 summary — P0 (1.1.0) scope

Effort S/M/L; agent value H/M/L. Sorted by value then cost. This is the implementation
train's scope, self-evident at a glance.

| # | P0 item | Agent value | Effort | Why it earns its place |
|---|---|---|---|---|
| 1 | Default machine JSON + `--human` opt-in (no TTY detection) | **H** | M | Flagship: deterministic machine output with zero flags — the core agent-native win. |
| 2 | Error record `{code,message,violations?}`+`type`+`cli` | **H** | M | The data contract every agent error path parses; AGENT-002-aligned. |
| 3 | Diagnostics JSONL + `meta` schema | **H** | M | Lets agents tell warnings from errors; prereq for the rest. |
| 4 | exit set `0–5` | **H** | S | `$?` branching; only code `5` is new. |
| 5 | `-c` hard-error + `-b` cookie-send | **H** | S | Kills a silent-wrong dangerous trap. |
| 6 | `-d @file` / `-d @-` / one-body-error | **H** | S | How agents feed generated bodies; trivial. |
| 7 | did-you-mean (introspection) + invoke `cli.hint` (schema/discover pointers) | **H** | M | Closes the self-correction loop locally, no server change. |
| 8 | `--max-time` + `--connect-timeout` | **H** | M | Bounded waits — agents must not hang. |
| 9 | `--client-id/--client-meta` rename, `-H` colon, `--oauth2-bearer` | **M** | S | Frees shorts for agent-native reuse; cheap familiarity. |

**Rough sizing:** ~4 items S + ~5 items M ⇒ one focused implementation train (≈ an
RPCURL-001-scale change). P1 is independently shippable afterward, prioritized by the §3.2
value/effort column.

---

## Appendix A — Current-code observations (NOT fixed here, per guardrails)

- **A-1**: `-d` beats `-f` silently (`args.rs:114-121`); §1.1 replaces this with a
  one-body usage error.
- **A-2**: malformed `-H` warned-and-ignored to plain stderr (`args.rs:156`,
  `main.rs:108-111`); §1.2/§2.3 keep tolerance but move it to the structured channel.
- **A-3**: introspection is http-only (`discover.rs:425-429`); TLS servers can't be
  discovered. https support = P1.
- **A-4**: README shows `cargo run` examples and old `-i/-m` names (README:12-38); update
  belongs to the implementation train.
- **A-5**: success envelope `code` is numeric (`output.rs:24-38`) while the error
  record's `code` is a string (§2.2) — distinct namespaces, documented, not merged.

## Appendix B — curl facts to verify before implementation

- `-d @file` newline handling vs `--data-raw`/`--data-binary` `[curl:verify]`.
- `-H 'Name;'` empty-header / no-colon handling `[curl:verify]`.
- (Exit numbers, `--json` floor, `-w` grammar are no longer targets — cut from scope.)
