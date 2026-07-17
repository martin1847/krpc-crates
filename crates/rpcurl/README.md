# rpcurl

Agent-native command-line client for the KRPC wire (JSON-in-gRPC), plus plain-HTTP
introspection. curl is a *familiarity reference*, not a compatibility target: where a
curl convention transfers for free, rpcurl uses it; otherwise it optimizes for agents.

## Invoke

```bash
export KRPC_TOKEN=env.token
export DEMO="http://127.0.0.1:50051/demo-java-server/Demo"

rpcurl $DEMO/hello -d '{"name":"KRPC","age":28}'   # inline JSON body
rpcurl $DEMO/hello -d @body.json                    # body from a file
echo '{"name":"KRPC"}' | rpcurl $DEMO/hello -d @-   # body from stdin
rpcurl $DEMO/bytesTime                               # no body (defaults to null)
```

## Output: machine JSON by default, `--human` to decorate

**Machine JSON is the unconditional default** — bare JSON payload on stdout, JSONL
diagnostics on stderr, **with no TTY detection**. You get identical output in a terminal
or a pipe (deterministic; the agent path). **`--human`** opts into decorated output.

```bash
rpcurl $DEMO/hello -d '{"name":"KRPC"}'            # -> {"code":0,"data":{...}} (JSON)
rpcurl $DEMO/hello -d '{"name":"KRPC"}' | jq       # same JSON, deterministic
rpcurl --human $DEMO/hello -d '{"name":"KRPC"}'    # -> 🍀 pretty, decorated
```

- Success payload → **stdout**. Diagnostics and errors → **stderr**. Mode controls
  decoration only, never routing.
- By default, stderr is **JSONL**: one JSON object per line, each with a `type`
  (`"warning"` | `"error"` | `"debug"`), so a warning or `-v` debug line is
  distinguishable from the terminal error.
- `-v/--verbose` adds framed `type:"debug"` records in machine mode; values for
  `authorization`, `cookie`, `c-id` (defaults to `r-<hostname>`) and `c-meta` are
  redacted (wire headers unchanged). Decorated blocks in human mode.
- `--help` / `--version` obey the mode too. Default (machine): `--version` →
  `{"name","version"}`; `--help` → **structured** self-description derived from the CLI
  model — `{name, version, usage, args[], subcommands[], exit_codes{}}` where each arg has
  `{name, short, aliases[], takes_value, value_name, doc}` (`--oauth2-bearer` is an alias
  of `--token`; `value_name` is null for flags). A real introspection surface, like
  `discover`/`schema`. `--human` prints clap's text. Exit 0.

## Flags

| Flag | Meaning |
|---|---|
| `-d, --data <json>` | Request body. `@path` = file, `@-` = stdin, else verbatim JSON. |
| `-t, --token <t>` / `--oauth2-bearer <t>` | `Authorization: Bearer <t>` (env `KRPC_TOKEN`). |
| `-b, --cookie <c>` | Send a cookie header (env `KRPC_COOKIE`). *(curl's `-b`.)* |
| `-H, --header 'Name: value'` | Custom header; `k=v` also accepted. Repeatable. |
| `--client-id <id>` | `c-id` tracking header (env `KRPC_CID`; default `r-<hostname>`). |
| `--client-meta <m>` | `c-meta` header (env `KRPC_CMETA`). |
| `-m, --max-time <s>` | Overall deadline (seconds) across connect + call. |
| `--connect-timeout <s>` | Connection-phase deadline (seconds). |
| `--human` | Decorated (emoji/pretty) output; default is machine JSON. |
| `-v, --verbose` | Debug diagnostics to stderr. |

> `-c` is **not** a cookie flag here. curl's `-c` writes a cookie *jar* (rpcurl has none),
> so `rpcurl -c` is a hard error pointing you at `-b`. This avoids a dangerous curl trap.

## Exit codes

A small, stable set for `$?` branching:

| code | meaning |
|------|---------|
| 0 | success (data on stdout) |
| 1 | remote error (server reachable, returned a gRPC/app error) |
| 2 | usage error (bad url / json / file / args, incl. parse errors) |
| 3 | connect error (could not reach the endpoint) |
| 4 | protocol error (reached, non-2xx / unparseable body — introspection) |
| 5 | timeout (`--connect-timeout` / `--max-time` exceeded) |

## Machine error record

On error, machine mode emits one JSONL object on stderr. The top level is the ecosystem
error triplet `{code, message, violations?}` (aligned with the MCP/AGENT-002 envelope);
CLI-only fields live under `cli`:

```json
{"type":"error","code":"INVALID_ARGUMENT","message":"name: must not be blank","cli":{"kind":"remote","exit":1,"grpcCode":3,"hint":"run `rpcurl schema <host> Svc/method` to see the input schema"}}
```

- `code` is a string: the `google.rpc.Code` name for a remote error, else the CLI kind
  (`USAGE`/`CONNECT`/`PROTOCOL`/`TIMEOUT`).
- `violations` is omitted until the server publishes a versioned Status-detail schema
  (graceful degradation).
- On an `INVALID_ARGUMENT`, `cli.hint` points at `rpcurl schema` for self-correction.

## Introspection

Plain HTTP against the server's `/agent/discover`:

```bash
rpcurl discover http://127.0.0.1:50051                 # list services/methods
rpcurl schema   http://127.0.0.1:50051 Hello/hello     # JSON schema for one method
rpcurl example  http://127.0.0.1:50051 Hello/hello     # skeleton input JSON
```

All use the default machine mode; `--human` opts into decorated output. A mistyped method suggests the nearest known one (`did you mean …?`).

## Self-correction loop (agents)

```bash
rpcurl schema  $HOST Hello/hello | jq .input          # learn the input shape
rpcurl example $HOST Hello/hello                       # get a skeleton
rpcurl $HOST/Hello/hello -d '{"age":28}'              # missing field -> exit 1 + hint
rpcurl $HOST/Hello/hello -d '{"name":"KRPC","age":28}' # fixed -> exit 0
```
