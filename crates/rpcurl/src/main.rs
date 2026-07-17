mod args;
mod discover;
mod error;
mod output;

use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;
use krpc::{clt::KrpcClient, proto::Out};

use args::{Cli, Command};
use error::CliError;
use output::{render_error, render_success, Mode};

#[tokio::main]
async fn main() -> ExitCode {
    // `--json` is a global flag; detect it from raw argv so that clap parse
    // failures (which occur before we can read the parsed `Cli`) still honor the
    // machine-parseable error contract.
    let json_argv = std::env::args().any(|a| a == "--json");

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => return handle_parse_error(e, json_argv),
    };
    let mode = if cli.json { Mode::Json } else { Mode::Human };

    let stdout = io::stdout();
    let mut out = stdout.lock();

    match run(&cli, mode, &mut out).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // ALL errors go to fd2 (real stderr), never stdout, with a non-zero code.
            let stderr = io::stderr();
            let mut w = stderr.lock();
            let _ = render_error(&mut w, mode, &e);
            ExitCode::from(e.exit_code())
        }
    }
}

/// Handle a clap parse outcome. Help/version are not errors — preserve clap's
/// normal stdout output and exit 0. Genuine parse errors are usage errors
/// (exit 2): in `--json` mode they follow the JSON error contract on stderr;
/// otherwise clap's human message is printed to stderr as before.
fn handle_parse_error(e: clap::Error, json: bool) -> ExitCode {
    use clap::error::ErrorKind;
    if matches!(
        e.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        let _ = e.print();
        return ExitCode::SUCCESS;
    }
    if json {
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
        let _ = writeln!(diag, "[krpc url]:\n{http_host}{method_path}\n");
    }

    let json = inv.read_data()?;
    if verbose {
        let _ = writeln!(diag, "[request json]:\n{json}\n");
    }

    let (req, warnings) = inv.build_request(json)?;
    for kv in warnings {
        let _ = writeln!(diag, "ignore malformed -H [ {kv} ]");
    }
    if verbose {
        let _ = writeln!(diag, "[request headers]:\n{:?}\n", req.metadata());
    }

    let mut client = KrpcClient::connect(http_host)
        .await
        .map_err(|e| CliError::Connect(error::chain(&e)))?;

    let response = client.call(&method_path, req).await.map_err(|status| {
        CliError::Remote {
            code: status.code() as i32,
            msg: status.message().to_owned(),
        }
    })?;

    if verbose {
        let _ = writeln!(diag, "[response headers]:\n{:?}\n", response.metadata());
        let ext = response.extensions();
        if !ext.is_empty() {
            let _ = writeln!(diag, "[response extensions]:\n{ext:?}\n");
        }
    }

    let res = response.get_ref();
    // Application-level error carried inside a successful gRPC response: route to
    // stderr with a non-zero exit code (was previously stdout + exit 0).
    if let Some(err) = response_error(res) {
        return Err(err);
    }
    if verbose && let Some(Out::Json(json)) = &res.out {
        let _ = writeln!(diag, "[response raw json]:\n{json}\n");
    }

    render_success(out, mode, res).map_err(|e| CliError::Usage(format!("write error: {e}")))
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
}
