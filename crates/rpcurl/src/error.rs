//! CLI error taxonomy with agent-friendly, stable exit codes.
//!
//! Every non-success path in rpcurl resolves to one of these variants. `main`
//! renders it to **stderr** (fd2) — never stdout — and returns the matching
//! non-zero exit code, so a machine consumer can branch on `$?` alone:
//!
//! | code | meaning                                                      |
//! |------|--------------------------------------------------------------|
//! | 0    | success (data on stdout)                                     |
//! | 1    | remote error: server reachable, returned an error           |
//! | 2    | usage error: bad CLI input (url / json / file / args)        |
//! | 3    | connect error: could not establish a connection             |
//! | 4    | protocol error: server reached but replied non-2xx / body    |
//! |      | could not be parsed (introspection over HTTP)                |
//! | 5    | timeout: `--connect-timeout` / `--max-time` exceeded         |

/// Flatten an error and its `source()` chain into one message, e.g.
/// `transport error: tcp connect error: Connection refused (os error 61)`.
/// tonic's transport `Display` alone is just "transport error"; the actionable
/// cause lives in the source chain, which agents need to self-correct.
pub(crate) fn chain(err: &dyn std::error::Error) -> String {
    let mut msg = err.to_string();
    let mut src = err.source();
    while let Some(e) = src {
        let s = e.to_string();
        if !s.is_empty() && !msg.ends_with(&s) {
            msg.push_str(": ");
            msg.push_str(&s);
        }
        src = e.source();
    }
    msg
}

/// Canonical `google.rpc.Code` name for a numeric gRPC status code, e.g.
/// `3 → "INVALID_ARGUMENT"`. Unknown numbers stringify as `CODE_<n>` so the
/// machine `code` field stays a stable string (never a bare int).
pub(crate) fn grpc_code_name(code: i32) -> String {
    let name = match code {
        0 => "OK",
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        4 => "DEADLINE_EXCEEDED",
        5 => "NOT_FOUND",
        6 => "ALREADY_EXISTS",
        7 => "PERMISSION_DENIED",
        8 => "RESOURCE_EXHAUSTED",
        9 => "FAILED_PRECONDITION",
        10 => "ABORTED",
        11 => "OUT_OF_RANGE",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        15 => "DATA_LOSS",
        16 => "UNAUTHENTICATED",
        n => return format!("CODE_{n}"),
    };
    name.to_owned()
}

/// A fatal CLI error. Carries just enough to render a clean message and pick an
/// exit code; the `Debug`-formatted transport internals never leak to output.
#[derive(Debug)]
pub(crate) enum CliError {
    /// Bad CLI input: malformed url, unparseable json, missing file, bad args.
    Usage(String),
    /// Could not establish a connection to the endpoint.
    Connect(String),
    /// Server was reachable but returned an error — either a gRPC `Status` or an
    /// application-level `Out::Error`. `code` is the gRPC/`google.rpc.Code`.
    /// `hint` is an optional self-correction pointer (e.g. a `rpcurl schema`
    /// command) surfaced under `cli.hint` in machine mode.
    Remote {
        code: i32,
        msg: String,
        hint: Option<String>,
    },
    /// Server was reached over HTTP but replied with a non-2xx status or a body
    /// that could not be parsed (introspection endpoints). Distinct from
    /// [`CliError::Connect`]: the endpoint is up, but the exchange failed.
    Protocol(String),
    /// A `--connect-timeout` / `--max-time` deadline elapsed before the call
    /// completed.
    Timeout(String),
}

impl CliError {
    /// Process exit code for this error. See the table in the module docs.
    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            CliError::Remote { .. } => 1,
            CliError::Usage(_) => 2,
            CliError::Connect(_) => 3,
            CliError::Protocol(_) => 4,
            CliError::Timeout(_) => 5,
        }
    }

    /// Machine-stable error kind, used as `cli.kind` in machine mode.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            CliError::Usage(_) => "usage",
            CliError::Connect(_) => "connect",
            CliError::Remote { .. } => "remote",
            CliError::Protocol(_) => "protocol",
            CliError::Timeout(_) => "timeout",
        }
    }

    /// Human-readable message.
    pub(crate) fn message(&self) -> &str {
        match self {
            CliError::Usage(m)
            | CliError::Connect(m)
            | CliError::Protocol(m)
            | CliError::Timeout(m) => m,
            CliError::Remote { msg, .. } => msg,
        }
    }

    /// Stable string `code` for the machine error record: the `google.rpc.Code`
    /// name for a remote error, else the uppercased kind.
    pub(crate) fn code_str(&self) -> String {
        match self {
            CliError::Remote { code, .. } => grpc_code_name(*code),
            other => other.kind().to_ascii_uppercase(),
        }
    }

    /// gRPC/`google.rpc.Code` for a remote error, else `None`.
    pub(crate) fn remote_code(&self) -> Option<i32> {
        match self {
            CliError::Remote { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// Self-correction hint (e.g. a `rpcurl schema` command), if any.
    pub(crate) fn hint(&self) -> Option<&str> {
        match self {
            CliError::Remote { hint, .. } => hint.as_deref(),
            _ => None,
        }
    }
}
