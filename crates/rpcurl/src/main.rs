mod args;
mod discover;
mod error;
mod output;

use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;
use krpc::{clt::KrpcClient, proto::Out};
use serde_json::json;

use args::{Cli, Command, InvokeArgs};
use error::CliError;
use output::{render_debug, render_error, render_success, render_warning, Mode};

#[tokio::main]
async fn main() -> ExitCode {
    // Machine JSON is the unconditional default (no TTY detection); `--human` opts
    // into decoration. Resolve it from raw argv too, so clap parse failures (before
    // we can read the parsed `Cli`) honor the same rule.
    let argv_mode = if std::env::args().skip(1).any(|a| a == "--human") {
        Mode::Human
    } else {
        Mode::Json
    };

    // The `-c` trap: curl's cookie-JAR (save) flag. rpcurl has no jar, so reject
    // it — even bare `-c` — with a corrective hard error before clap's generic
    // value handling can turn it into an opaque "requires a value" message.
    if std::env::args().skip(1).any(|a| args::is_cookie_jar_flag(&a)) {
        let stderr = io::stderr();
        let mut w = stderr.lock();
        let _ = render_error(&mut w, argv_mode, &CliError::Usage(args::COOKIE_JAR_MSG.to_owned()));
        return ExitCode::from(2);
    }

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => return handle_parse_error(e, argv_mode),
    };
    let mode = if cli.human { Mode::Human } else { Mode::Json };

    let stdout = io::stdout();
    let mut out = stdout.lock();

    match run(&cli, mode, &mut out).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let stderr = io::stderr();
            let mut w = stderr.lock();
            let _ = render_error(&mut w, mode, &err);
            ExitCode::from(err.exit_code())
        }
    }
}

/// Handle a clap parse outcome. Help/version are not errors — preserve clap's
/// normal stdout output and exit 0. Genuine parse errors are usage errors
/// (exit 2): in machine mode they follow the JSONL error contract on stderr;
/// otherwise clap's human message is printed to stderr as before.
fn handle_parse_error(e: clap::Error, mode: Mode) -> ExitCode {
    use clap::error::ErrorKind;
    if matches!(
        e.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        return emit_help(&e, mode);
    }
    if e.kind() == ErrorKind::DisplayVersion {
        return emit_version(mode);
    }
    if mode == Mode::Json {
        // Collapse clap's multi-line usage text to a single-line summary.
        let full = e.to_string();
        let msg = full
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("argument error")
            .trim_start_matches("error: ")
            .to_owned();
        let stderr = io::stderr();
        let mut w = stderr.lock();
        let _ = render_error(&mut w, Mode::Json, &CliError::Usage(msg));
    } else {
        let _ = e.print();
    }
    ExitCode::from(2)
}

/// Emit clap help in the resolved mode: machine mode wraps the rendered help in a
/// `{"help": …}` JSON object on stdout; human prints clap's text. Exit 0.
fn emit_help(e: &clap::Error, mode: Mode) -> ExitCode {
    let stdout = io::stdout();
    let mut w = stdout.lock();
    match mode {
        Mode::Json => {
            let obj = json!({ "help": e.render().to_string() });
            let _ = writeln!(w, "{obj}");
        }
        Mode::Human => {
            let _ = e.print();
        }
    }
    ExitCode::SUCCESS
}

/// Emit the version in the resolved mode: `{"name","version"}` JSON on stdout, or
/// `name version` text. Exit 0.
fn emit_version(mode: Mode) -> ExitCode {
    let stdout = io::stdout();
    let mut w = stdout.lock();
    match mode {
        Mode::Json => {
            let obj = json!({ "name": env!("CARGO_PKG_NAME"), "version": env!("CARGO_PKG_VERSION") });
            let _ = writeln!(w, "{obj}");
        }
        Mode::Human => {
            let _ = writeln!(w, "{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        }
    }
    ExitCode::SUCCESS
}

async fn run(cli: &Cli, mode: Mode, out: &mut impl Write) -> Result<(), CliError> {
    match &cli.command {
        Some(Command::Discover { base_url }) => discover::run_discover(out, mode, base_url).await,
        Some(Command::Schema { base_url, method }) => {
            discover::run_schema(out, mode, base_url, method).await
        }
        Some(Command::Example { base_url, method }) => {
            discover::run_example(out, mode, base_url, method).await
        }
        None => invoke(cli, mode, out).await,
    }
}

/// Default path: invoke an RPC over the KRPC gRPC wire.
async fn invoke(cli: &Cli, mode: Mode, out: &mut impl Write) -> Result<(), CliError> {
    let inv = &cli.invoke;
    let verbose = cli.verbose;
    // Diagnostics go to stderr so stdout stays pure data even under -v.
    let stderr = io::stderr();
    let mut diag = stderr.lock();

    let (http_host, method_path) = inv.split_url()?;
    if verbose {
        let _ = render_debug(&mut diag, mode, "krpc url", &format!("{http_host}{method_path}"));
    }

    let json = inv.read_data()?;
    if verbose {
        let _ = render_debug(&mut diag, mode, "request json", &json);
    }

    let (req, warnings) = inv.build_request(json)?;
    for kv in warnings {
        let _ = render_warning(
            &mut diag,
            mode,
            "W_HEADER_MALFORMED",
            &format!("ignoring malformed -H (no ':' or '='): {kv}"),
        );
    }
    if verbose {
        let _ = render_debug(&mut diag, mode, "request headers", &dump_metadata(req.metadata()));
    }

    let connect_to = inv.connect_timeout()?;
    let max_to = inv.max_time()?;

    let host_for_connect = http_host.clone();
    let net = async move {
        let connect_fut = KrpcClient::connect(host_for_connect);
        let client_res = match connect_to {
            Some(d) => tokio::time::timeout(d, connect_fut).await.map_err(|_| {
                CliError::Timeout(format!("connect timeout after {:.3}s", d.as_secs_f64()))
            })?,
            None => connect_fut.await,
        };
        let mut client = client_res.map_err(|e| CliError::Connect(error::chain(&e)))?;
        client
            .call(&method_path, req)
            .await
            .map_err(|status| CliError::Remote {
                code: status.code() as i32,
                msg: status.message().to_owned(),
                hint: None,
            })
    };

    let result = match max_to {
        Some(d) => match tokio::time::timeout(d, net).await {
            Ok(inner) => inner,
            Err(_) => Err(CliError::Timeout(format!(
                "max-time exceeded after {:.3}s",
                d.as_secs_f64()
            ))),
        },
        None => net.await,
    };
    let response = result.map_err(|e| with_hint(e, &http_host, inv))?;

    if verbose {
        let _ = render_debug(&mut diag, mode, "response headers", &dump_metadata(response.metadata()));
        let ext = response.extensions();
        if !ext.is_empty() {
            let _ = render_debug(&mut diag, mode, "response extensions", &format!("{ext:?}"));
        }
    }

    let res = response.get_ref();
    // Application-level error carried inside a successful gRPC response: route to
    // stderr with a non-zero exit code (was previously stdout + exit 0).
    if let Some(err) = response_error(res) {
        return Err(with_hint(err, &http_host, inv));
    }
    if verbose && let Some(Out::Json(json)) = &res.out {
        let _ = render_debug(&mut diag, mode, "response raw json", json);
    }

    render_success(out, mode, res).map_err(|e| CliError::Usage(format!("write error: {e}")))
}

/// Attach a self-correction hint to a remote error: `INVALID_ARGUMENT` → the
/// `rpcurl schema` command for the method; `UNIMPLEMENTED` (method not found) →
/// `rpcurl discover` to list methods. No-op otherwise.
fn with_hint(err: CliError, host: &str, inv: &InvokeArgs) -> CliError {
    match err {
        CliError::Remote { code: 3, msg, hint: None } => {
            let hint = inv
                .service_method()
                .map(|sm| format!("run `rpcurl schema {host} {sm}` to see the input schema"));
            CliError::Remote { code: 3, msg, hint }
        }
        CliError::Remote { code: 12, msg, hint: None } => CliError::Remote {
            code: 12,
            msg,
            hint: Some(format!("run `rpcurl discover {host}` to list available methods")),
        },
        other => other,
    }
}

/// Sensitive metadata keys whose values are redacted in `-v` dumps (the wire
/// header is unchanged). `authorization`/`cookie` carry secrets; `c-id` defaults
/// to `r-<hostname>` (an internal hostname — repo rule: never into logs); `c-meta`
/// is caller-supplied and may carry identifying values, so it is redacted too.
const REDACTED_META: &[&str] = &["authorization", "cookie", "c-id", "c-meta"];

/// Render gRPC metadata for `-v` with sensitive values redacted so verbose dumps
/// never leak bearer tokens, session cookies, or the caller's hostname/identity.
fn dump_metadata(md: &tonic::metadata::MetadataMap) -> String {
    use tonic::metadata::KeyAndValueRef;
    let mut parts = Vec::new();
    for kv in md.iter() {
        match kv {
            KeyAndValueRef::Ascii(k, v) => {
                let key = k.as_str();
                let val = if REDACTED_META.contains(&key) {
                    "<redacted>"
                } else {
                    v.to_str().unwrap_or("<non-ascii>")
                };
                parts.push(format!("{key}: {val}"));
            }
            KeyAndValueRef::Binary(k, _) => parts.push(format!("{}: <binary>", k.as_str())),
        }
    }
    parts.join(", ")
}

/// Classify a (successful gRPC) response: an `Out::Error` payload is an
/// application-level failure and maps to a remote error (exit 1). Everything
/// else is success. Extracted so the regression for the historical
/// "Out::Error printed to stdout with exit 0" bug is directly testable.
fn response_error(res: &krpc::proto::OutputProto) -> Option<CliError> {
    match &res.out {
        Some(Out::Error(msg)) => Some(CliError::Remote {
            code: res.code,
            msg: msg.clone(),
            hint: None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use krpc::proto::OutputProto;

    #[test]
    fn out_error_maps_to_remote_exit_1() {
        let res = OutputProto {
            code: 9,
            out: Some(Out::Error("boom".to_owned())),
        };
        let err = response_error(&res).expect("Out::Error must be a failure, not success");
        assert_eq!(err.exit_code(), 1);
        assert_eq!(err.remote_code(), Some(9));
        assert_eq!(err.message(), "boom");
    }

    #[test]
    fn out_json_is_success() {
        let res = OutputProto {
            code: 0,
            out: Some(Out::Json("{}".to_owned())),
        };
        assert!(response_error(&res).is_none());
    }

    #[test]
    fn hint_targets_invalid_argument_and_unimplemented() {
        let inv = InvokeArgs::for_url("http://h:1/app/Svc/method");
        let schema = with_hint(
            CliError::Remote { code: 3, msg: "x".to_owned(), hint: None },
            "http://h:1",
            &inv,
        );
        assert!(schema.hint().unwrap().contains("schema"), "INVALID_ARGUMENT -> schema hint");
        let discover = with_hint(
            CliError::Remote { code: 12, msg: "x".to_owned(), hint: None },
            "http://h:1",
            &inv,
        );
        assert!(discover.hint().unwrap().contains("discover"), "UNIMPLEMENTED -> discover hint");
        let none = with_hint(
            CliError::Remote { code: 5, msg: "x".to_owned(), hint: None },
            "http://h:1",
            &inv,
        );
        assert!(none.hint().is_none(), "NOT_FOUND gets no hint");
    }
}
