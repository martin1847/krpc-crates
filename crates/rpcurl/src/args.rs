use crate::error::CliError;
use krpc::proto::InputProto;
use serde_json::{from_str, Value};
use std::io::Read;
use std::time::Duration;

/// rpcurl — agent-native command-line KRPC client.
///
/// Default (no subcommand) invokes an RPC: `rpcurl <url> -d '<json>'`. The
/// introspection subcommands (`discover`, `schema`, `example`) talk plain HTTP
/// to the server's `/agent/discover` endpoint instead.
#[derive(clap::Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    #[command(flatten)]
    pub(crate) invoke: InvokeArgs,

    /// Human mode: emoji-decorated payload + decorated stderr diagnostics. Default
    /// (this flag absent) is machine JSON on stdout + JSONL diagnostics on stderr,
    /// unconditionally (no TTY detection) — deterministic in a terminal or a pipe.
    #[arg(long, global = true, action = clap::ArgAction::SetTrue)]
    pub(crate) human: bool,

    /// Verbose diagnostics (url, input, headers) to stderr — a human debug aid.
    #[arg(short, long, global = true, action = clap::ArgAction::SetTrue)]
    pub(crate) verbose: bool,
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum Command {
    /// List services and methods exposed by a server (via GET /agent/discover).
    Discover {
        /// Server base URL, e.g. `http://127.0.0.1:50051`
        base_url: String,
    },
    /// Show the input/output JSON schema for one `Service/method`.
    Schema {
        /// Server base URL, e.g. `http://127.0.0.1:50051`
        base_url: String,
        /// Method path, e.g. `Hello/hello`
        method: String,
    },
    /// Generate a skeleton input JSON (placeholder values) for one method.
    Example {
        /// Server base URL, e.g. `http://127.0.0.1:50051`
        base_url: String,
        /// Method path, e.g. `Hello/hello`
        method: String,
    },
}

#[derive(clap::Args, Debug, Default)]
pub(crate) struct InvokeArgs {
    /// RPC url, e.g. https://demo.krpc.tech/appName/Service/method
    pub(crate) url: Option<String>,

    /// Request body JSON. `@path` reads a file, `@-` reads stdin, otherwise the
    /// value is used verbatim. e.g. `-d '{"name":"KRPC"}'`, `-d @body.json`.
    #[arg(short, long)]
    data: Option<String>,

    /// Bearer token → `Authorization: Bearer <token>`. Alias: `--oauth2-bearer`.
    #[arg(short = 't', long, visible_alias = "oauth2-bearer", env = "KRPC_TOKEN")]
    token: Option<String>,

    /// Send a cookie header, e.g. `-b 'tk=j.w.t'`. (curl's `-b`.)
    #[arg(short = 'b', long, env = "KRPC_COOKIE")]
    cookie: Option<String>,

    /// Client id for request tracking → `c-id` header (default `r-<hostname>`).
    #[arg(long, env = "KRPC_CID")]
    client_id: Option<String>,

    /// Client meta → `c-meta` header.
    #[arg(long, env = "KRPC_CMETA")]
    client_meta: Option<String>,

    /// Custom headers, `Name: value` (curl) or `k=v`. Repeatable: `-H a:b -H c:d`.
    #[arg(short = 'H', long)]
    header: Option<Vec<String>>,

    /// Overall deadline in seconds across connect + call (curl's `-m`).
    #[arg(short = 'm', long)]
    max_time: Option<f64>,

    /// Connection-phase deadline in seconds.
    #[arg(long)]
    connect_timeout: Option<f64>,
}

const JSON_NULL: &str = "null";

/// Maximum request body read from a file/stdin (`-d @…`). Bounds memory against a
/// hostile or accidental unbounded pipe; the literal `-d` value is already bounded
/// by the OS argv limit.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

impl InvokeArgs {
    /// Split the RPC url into `(http_host, method_path)`.
    ///
    /// Robust against `http://` vs `https://`: locates the scheme separator, then
    /// the first `/` of the path. Returns a [`CliError::Usage`] for malformed urls
    /// instead of panicking.
    pub(crate) fn split_url(&self) -> Result<(String, String), CliError> {
        let url = self
            .url
            .as_deref()
            .ok_or_else(|| CliError::Usage("missing RPC url (or a subcommand)".to_owned()))?;
        let scheme_end = url
            .find("://")
            .map(|i| i + 3)
            .ok_or_else(|| CliError::Usage(format!("url must start with http:// or https:// < {url} >")))?;
        let slash = url[scheme_end..]
            .find('/')
            .ok_or_else(|| CliError::Usage(format!("url missing /app/Service/method path < {url} >")))?;
        let split = scheme_end + slash;
        Ok((url[..split].to_owned(), url[split..].to_owned()))
    }

    /// The trailing `Service/method` of the url path, for self-correction hints
    /// (e.g. `/quickstart/Hello/hello` → `Hello/hello`).
    pub(crate) fn service_method(&self) -> Option<String> {
        let (_, path) = self.split_url().ok()?;
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        match segs.as_slice() {
            [.., svc, method] => Some(format!("{svc}/{method}")),
            _ => None,
        }
    }

    /// Resolve and validate the request JSON body. `-d @path` reads a file,
    /// `-d @-` reads stdin, otherwise the value is verbatim; defaults to JSON
    /// `null`. File/stdin reads are capped at [`MAX_BODY_BYTES`]; a parse error
    /// reports the byte length + parser location only — **never the raw body** —
    /// so a token/PII payload with a trailing syntax error is not echoed into
    /// diagnostics or CI logs. Errors surface as [`CliError::Usage`].
    pub(crate) fn read_data(&self) -> Result<String, CliError> {
        let json_data = match self.data.as_deref() {
            Some("@-") => read_capped(std::io::stdin().lock(), "stdin")?,
            Some(spec) if spec.starts_with('@') => {
                let path = &spec[1..];
                let f = std::fs::File::open(path)
                    .map_err(|e| CliError::Usage(format!("cannot open file < {path} >: {e}")))?;
                read_capped(f, &format!("file < {path} >"))?
            }
            Some(literal) => literal.to_owned(),
            None => JSON_NULL.to_owned(),
        };
        from_str::<Value>(&json_data).map_err(|e| {
            CliError::Usage(format!("invalid request json ({} bytes): {e}", json_data.len()))
        })?;
        Ok(json_data)
    }

    /// Overall deadline, if `--max-time` is set. Rejects non-positive values.
    pub(crate) fn max_time(&self) -> Result<Option<Duration>, CliError> {
        to_duration(self.max_time, "--max-time")
    }

    /// Connection-phase deadline, if `--connect-timeout` is set.
    pub(crate) fn connect_timeout(&self) -> Result<Option<Duration>, CliError> {
        to_duration(self.connect_timeout, "--connect-timeout")
    }

    /// Build the tonic request with headers. Returns the request plus any
    /// warnings (e.g. malformed `-H` entries) for the caller to report to stderr.
    pub(crate) fn build_request(
        &self,
        json: String,
    ) -> Result<(tonic::Request<InputProto>, Vec<String>), CliError> {
        let mut req = tonic::Request::new(InputProto { json });
        let warnings = self.fill_headers(req.metadata_mut())?;
        Ok((req, warnings))
    }

    fn fill_headers(
        &self,
        header: &mut tonic::metadata::MetadataMap,
    ) -> Result<Vec<String>, CliError> {
        use std::str::FromStr;
        let mut warnings = Vec::new();

        if let Some(list) = &self.header {
            for kv in list {
                // curl-style `Name: value` takes precedence; `k=v` is also
                // accepted. Header names are lowercased (case-insensitive on the
                // wire; tonic metadata keys must be ascii-lowercase).
                let parsed = kv
                    .split_once(':')
                    .map(|(k, v)| (k.trim(), v.trim_start()))
                    .or_else(|| kv.split_once('=').map(|(k, v)| (k.trim(), v)));
                let Some((k, v)) = parsed else {
                    warnings.push(kv.clone());
                    continue;
                };
                let key = tonic::metadata::MetadataKey::from_str(&k.to_ascii_lowercase())
                    .map_err(|e| CliError::Usage(format!("bad header key < {k} >: {e}")))?;
                let val = v
                    .parse()
                    .map_err(|e| CliError::Usage(format!("bad header value < {v} >: {e}")))?;
                header.insert(key, val);
            }
        }

        let cid: String = if let Some(client_id) = &self.client_id {
            client_id.clone()
        } else {
            format!("r-{}", gethostname::gethostname().to_str().unwrap_or("unknown"))
        };
        header.insert(
            "c-id",
            cid.parse()
                .map_err(|e| CliError::Usage(format!("bad c-id < {cid} >: {e}")))?,
        );

        if let Some(s) = &self.client_meta {
            header.insert(
                "c-meta",
                s.parse()
                    .map_err(|e| CliError::Usage(format!("bad c-meta: {e}")))?,
            );
        }
        if let Some(s) = &self.token {
            header.insert(
                "authorization",
                format!("Bearer {s}")
                    .parse()
                    .map_err(|e| CliError::Usage(format!("bad token: {e}")))?,
            );
        }
        if let Some(s) = &self.cookie {
            header.insert(
                "cookie",
                s.parse()
                    .map_err(|e| CliError::Usage(format!("bad cookie: {e}")))?,
            );
        }

        Ok(warnings)
    }
}

/// Convert `Option<seconds>` to `Option<Duration>`, rejecting negative/NaN.
fn to_duration(secs: Option<f64>, flag: &str) -> Result<Option<Duration>, CliError> {
    match secs {
        None => Ok(None),
        Some(s) if s.is_finite() && s >= 0.0 => Ok(Some(Duration::from_secs_f64(s))),
        Some(s) => Err(CliError::Usage(format!("{flag} must be >= 0 seconds, got {s}"))),
    }
}

/// Read at most [`MAX_BODY_BYTES`] of UTF-8 from `r` (a file or stdin). Rejects
/// non-UTF-8 input (the error text carries no raw bytes) and oversized input
/// (summarized, never copied) — safe for a default agent pipeline.
fn read_capped(r: impl Read, src: &str) -> Result<String, CliError> {
    let mut buf = String::new();
    let n = r
        .take((MAX_BODY_BYTES as u64) + 1)
        .read_to_string(&mut buf)
        .map_err(|e| CliError::Usage(format!("cannot read {src}: {e}")))?;
    if n > MAX_BODY_BYTES {
        return Err(CliError::Usage(format!(
            "{src} body exceeds the {MAX_BODY_BYTES}-byte cap"
        )));
    }
    Ok(buf)
}

/// Message for the `-c`/`--cookie-jar` trap (curl's save-jar flag; rpcurl has no
/// jar). Shared by the argv pre-scan and its test.
pub(crate) const COOKIE_JAR_MSG: &str =
    "`-c`/`--cookie-jar` is curl's cookie-jar (save) flag; rpcurl has no cookie jar. \
     To send a cookie use `-b`/`--cookie`.";

/// True if `arg` is the `-c`/`--cookie-jar` flag in any form (`-c`, `-cVALUE`,
/// `--cookie-jar`, `--cookie-jar=VALUE`). Scanned from raw argv so a **bare** `-c`
/// yields the corrective hard error before clap's generic value handling.
pub(crate) fn is_cookie_jar_flag(arg: &str) -> bool {
    arg == "--cookie-jar"
        || arg.starts_with("--cookie-jar=")
        || (arg.starts_with("-c") && !arg.starts_with("--"))
}

/// Test-only constructor: an [`InvokeArgs`] with just the url set (private
/// fields are unreachable from other modules' `..Default::default()`).
#[cfg(test)]
impl InvokeArgs {
    pub(crate) fn for_url(url: &str) -> Self {
        Self {
            url: Some(url.to_owned()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(url: &str) -> InvokeArgs {
        InvokeArgs {
            url: Some(url.to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn split_url_http() {
        let (host, path) = args("http://127.0.0.1:50051/quickstart/Hello/hello")
            .split_url()
            .unwrap();
        assert_eq!(host, "http://127.0.0.1:50051");
        assert_eq!(path, "/quickstart/Hello/hello");
    }

    #[test]
    fn split_url_https() {
        let (host, path) = args("https://demo.krpc.tech/app/Svc/method")
            .split_url()
            .unwrap();
        assert_eq!(host, "https://demo.krpc.tech");
        assert_eq!(path, "/app/Svc/method");
    }

    #[test]
    fn split_url_no_scheme_is_usage_error() {
        let err = args("127.0.0.1/app/Svc/m").split_url().unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn split_url_no_path_is_usage_error() {
        let err = args("http://127.0.0.1:50051").split_url().unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn service_method_takes_last_two_segments() {
        assert_eq!(
            args("http://h:1/quickstart/Hello/hello").service_method().as_deref(),
            Some("Hello/hello")
        );
    }

    #[test]
    fn read_data_defaults_to_null() {
        assert_eq!(args("http://h/a/S/m").read_data().unwrap(), "null");
    }

    #[test]
    fn read_data_rejects_bad_json() {
        let a = InvokeArgs {
            url: Some("http://h/a/S/m".to_owned()),
            data: Some("{not json".to_owned()),
            ..Default::default()
        };
        assert_eq!(a.read_data().unwrap_err().exit_code(), 2);
    }

    #[test]
    fn read_data_at_file_reads_and_validates() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("rpcurl_test_{}.json", std::process::id()));
        std::fs::write(&path, r#"{"name":"KRPC"}"#).unwrap();
        let a = InvokeArgs {
            url: Some("http://h/a/S/m".to_owned()),
            data: Some(format!("@{}", path.display())),
            ..Default::default()
        };
        assert_eq!(a.read_data().unwrap(), r#"{"name":"KRPC"}"#);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_data_at_missing_file_is_usage_error() {
        let a = InvokeArgs {
            url: Some("http://h/a/S/m".to_owned()),
            data: Some("@/no/such/rpcurl/file.json".to_owned()),
            ..Default::default()
        };
        assert_eq!(a.read_data().unwrap_err().exit_code(), 2);
    }

    #[test]
    fn cookie_jar_flag_detected_in_all_forms() {
        for a in ["-c", "-cjar.txt", "--cookie-jar", "--cookie-jar=jar.txt"] {
            assert!(is_cookie_jar_flag(a), "{a} must be detected as cookie-jar");
        }
        for a in ["-b", "--cookie", "--connect-timeout", "-d", "url"] {
            assert!(!is_cookie_jar_flag(a), "{a} must NOT be cookie-jar");
        }
        assert!(COOKIE_JAR_MSG.contains("-b"), "message points at -b");
    }

    #[test]
    fn header_accepts_colon_and_equals() {
        let a = InvokeArgs {
            url: Some("http://h/a/S/m".to_owned()),
            header: Some(vec!["X-Colon: cv".to_owned(), "x-eq=ev".to_owned()]),
            ..Default::default()
        };
        let (req, warnings) = a.build_request("null".to_owned()).unwrap();
        assert!(warnings.is_empty(), "both separators parse: {warnings:?}");
        let md = req.metadata();
        assert_eq!(md.get("x-colon").unwrap(), "cv");
        assert_eq!(md.get("x-eq").unwrap(), "ev");
    }

    #[test]
    fn header_value_may_contain_colon() {
        let a = InvokeArgs {
            url: Some("http://h/a/S/m".to_owned()),
            header: Some(vec!["authorization: Bearer a:b:c".to_owned()]),
            ..Default::default()
        };
        let (req, _) = a.build_request("null".to_owned()).unwrap();
        assert_eq!(req.metadata().get("authorization").unwrap(), "Bearer a:b:c");
    }

    #[test]
    fn malformed_header_becomes_warning_not_error() {
        let a = InvokeArgs {
            url: Some("http://h/a/S/m".to_owned()),
            header: Some(vec!["noseparator".to_owned(), "a=b".to_owned()]),
            ..Default::default()
        };
        let (_req, warnings) = a.build_request("null".to_owned()).unwrap();
        assert_eq!(warnings, vec!["noseparator".to_owned()]);
    }

    #[test]
    fn negative_timeout_is_usage_error() {
        let a = InvokeArgs {
            url: Some("http://h/a/S/m".to_owned()),
            max_time: Some(-1.0),
            ..Default::default()
        };
        assert_eq!(a.max_time().unwrap_err().exit_code(), 2);
    }
}
