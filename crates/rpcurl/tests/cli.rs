//! End-to-end CLI tests: run the compiled `rpcurl` binary and assert the
//! stdout/stderr split, exit codes, and the RPCURL-002 agent-native contracts.
//!
//! Output contract: **machine JSON is the unconditional default** (no TTY
//! detection); `--human` opts into decorated output. So every `run(...)` below
//! is machine mode unless it passes `--human`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output, Stdio};
use std::thread;

const BIN: &str = env!("CARGO_BIN_EXE_rpcurl");
const FIXTURE: &str = include_str!("fixtures/apimeta_quickstart.json");

fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().expect("spawn rpcurl")
}

fn run_stdin(args: &[&str], input: &str) -> Output {
    run_stdin_bytes(args, input.as_bytes())
}

/// Feed raw bytes to the child's stdin on a writer thread, so a child that stops
/// reading (e.g. at the body cap) yields a tolerated BrokenPipe rather than a hang.
fn run_stdin_bytes(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(BIN)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rpcurl");
    let mut stdin = child.stdin.take().unwrap();
    let buf = input.to_vec();
    let writer = thread::spawn(move || {
        let _ = stdin.write_all(&buf); // best-effort: child may stop at the cap
    });
    let out = child.wait_with_output().expect("wait rpcurl");
    let _ = writer.join();
    out
}

/// Serve a fixed HTTP `status` line and `body` for any request from a throwaway
/// localhost server. Returns `http://127.0.0.1:<port>`.
fn spawn_http(status: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn spawn_discover_server(body: &'static str) -> String {
    spawn_http("200 OK", body)
}

/// A listener that accepts connections and holds them open without ever speaking
/// HTTP/2 — the gRPC handshake hangs, so `--max-time` must fire.
fn spawn_blackhole() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let mut held = Vec::new();
        for s in listener.incoming().flatten() {
            held.push(s);
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// Parse the last stderr line as a JSONL diagnostics record (the terminal error).
fn stderr_json(out: &Output) -> serde_json::Value {
    let err = String::from_utf8_lossy(&out.stderr);
    let last = err.trim().lines().last().unwrap_or("");
    serde_json::from_str(last).unwrap_or_else(|e| panic!("stderr must be JSONL: {e}; got {err:?}"))
}

// --- output mode: default JSON, --human opts in --------------------------

// Flagship: default (no flag) is machine JSON, deterministic regardless of TTY.
#[test]
fn default_error_is_agent002_json() {
    let out = run(&["http://127.0.0.1:1/app/Svc/method", "-d", "{}"]);
    let v = stderr_json(&out);
    assert_eq!(v["type"], "error");
    assert_eq!(v["code"], "CONNECT");
    assert_eq!(v["cli"]["kind"], "connect");
    assert_eq!(v["cli"]["exit"], 3);
    assert!(v.get("msg").is_none(), "no legacy `msg` key");
    assert!(!String::from_utf8_lossy(&out.stderr).contains('❌'), "default = no decoration");
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn human_flag_error_is_decorated() {
    let out = run(&["--human", "http://127.0.0.1:1/app/Svc/method", "-d", "{}"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains('❌'), "--human decorates: {err}");
    assert!(!err.contains("\"type\""), "--human is not JSON");
    assert_eq!(out.status.code(), Some(3));
}

// --- error routing -------------------------------------------------------

#[test]
fn connect_error_goes_to_stderr_not_stdout() {
    let out = run(&["http://127.0.0.1:1/app/Svc/method", "-d", "{}"]);
    assert!(out.stdout.is_empty(), "stdout must be empty on error");
    assert!(!out.stderr.is_empty(), "stderr must carry the error");
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn bad_url_is_usage_error_exit_2() {
    let out = run(&["not-a-url", "-d", "{}"]);
    assert!(out.stdout.is_empty(), "no payload on stdout");
    assert!(!out.stderr.is_empty(), "must emit a diagnostic on stderr");
    let v = stderr_json(&out);
    assert_eq!(v["type"], "error");
    assert_eq!(v["cli"]["kind"], "usage");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn bad_json_input_is_usage_error_exit_2() {
    let out = run(&["http://127.0.0.1:1/app/Svc/method", "-d", "{bad"]);
    assert!(out.stdout.is_empty());
    assert_eq!(out.status.code(), Some(2));
}

// --- RPCURL-002 flags ----------------------------------------------------

#[test]
fn cookie_jar_c_is_hard_error_pointing_at_b() {
    let out = run(&["http://127.0.0.1:1/app/Svc/method", "-c", "tk=x", "-d", "{}"]);
    assert!(out.stdout.is_empty());
    assert_eq!(out.status.code(), Some(2), "-c is a hard usage error");
    let v = stderr_json(&out);
    assert!(v["message"].as_str().unwrap().contains("-b"), "must point at -b: {v}");
}

#[test]
fn bare_c_is_hard_error_pointing_at_b() {
    // The exact curl-user typo: a bare `-c` must give the corrective error, not a
    // generic clap "requires a value".
    let out = run(&["http://127.0.0.1:1/app/Svc/method", "-c", "-d", "{}"]);
    assert_eq!(out.status.code(), Some(2), "bare -c is a hard usage error");
    let v = stderr_json(&out);
    assert!(v["message"].as_str().unwrap().contains("-b"), "bare -c points at -b: {v}");
}

#[test]
fn data_at_stdin_is_read_and_validated() {
    // Invalid JSON on stdin proves `@-` was read (parse fails -> usage exit 2).
    let out = run_stdin(&["http://127.0.0.1:1/app/Svc/method", "-d", "@-"], "{invalid");
    assert_eq!(out.status.code(), Some(2), "stdin json validated");
    assert!(out.stdout.is_empty());
}

#[test]
fn data_at_stdin_valid_json_proceeds_to_connect() {
    let out = run_stdin(&["http://127.0.0.1:1/app/Svc/method", "-d", "@-"], "{\"x\":1}");
    assert_eq!(out.status.code(), Some(3), "valid stdin -> connect attempt -> exit 3");
}

#[test]
fn oversized_stdin_is_capped_usage_error() {
    let big = "a".repeat(9 * 1024 * 1024); // > 8 MiB cap
    let out = run_stdin(&["http://127.0.0.1:1/app/Svc/method", "-d", "@-"], &big);
    assert_eq!(out.status.code(), Some(2), "oversized stdin -> usage exit 2");
    let v = stderr_json(&out);
    assert!(v["message"].as_str().unwrap().contains("cap"), "reports the cap: {v}");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("aaaa"), "raw body not echoed");
}

#[test]
fn non_utf8_stdin_is_usage_error_without_raw_bytes() {
    let out = run_stdin_bytes(
        &["http://127.0.0.1:1/app/Svc/method", "-d", "@-"],
        &[0xff, 0xfe, 0x00, 0x01],
    );
    assert_eq!(out.status.code(), Some(2), "non-UTF8 stdin -> usage exit 2");
    let v = stderr_json(&out);
    assert_eq!(v["cli"]["kind"], "usage");
}

#[test]
fn max_time_timeout_is_exit_5() {
    let base = spawn_blackhole();
    let out = run(&[&format!("{base}/app/Svc/method"), "-d", "{}", "--max-time", "0.4"]);
    assert_eq!(
        out.status.code(),
        Some(5),
        "max-time -> timeout exit 5: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = stderr_json(&out);
    assert_eq!(v["code"], "TIMEOUT");
    assert_eq!(v["cli"]["kind"], "timeout");
}

// --- introspection (default JSON; --human for listing) -------------------

#[test]
fn discover_default_is_json() {
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["discover", &base]);
    assert!(out.status.success(), "{:?}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(!s.contains('🍀') && !s.contains('❌'));
    let v: serde_json::Value = serde_json::from_str(s.trim()).expect("default must be pure JSON");
    assert_eq!(v["app"], "quickstart");
    assert_eq!(v["services"][0]["methods"][0]["path"], "Hello/hello");
}

#[test]
fn discover_human_lists_methods() {
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["discover", &base, "--human"]);
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("Hello/hello("), "human listing: {s}");
}

#[test]
fn schema_json_has_input_and_output() {
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["schema", &base, "Hello/hello"]);
    assert!(out.status.success(), "{:?}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(v["input"]["properties"]["name"]["type"], "string");
    assert_eq!(v["output"]["title"], "HelloReply");
}

#[test]
fn schema_unknown_method_is_usage_error() {
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["schema", &base, "Hello/nope"]);
    assert!(out.stdout.is_empty());
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn schema_typo_suggests_nearest_method() {
    // did-you-mean: a 1-edit typo of a real method suggests it.
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["schema", &base, "Hello/helo"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("Hello/hello"), "should suggest the nearest method: {err}");
}

#[test]
fn example_produces_skeleton_json() {
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["example", &base, "Hello/hello"]);
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(v, serde_json::json!({ "name": "string" }));
}

// --- clap parse errors ---------------------------------------------------

#[test]
fn default_unknown_option_is_json_error_on_stderr() {
    let out = run(&["--not-a-real-option"]);
    assert!(out.stdout.is_empty(), "clap errors must not touch stdout");
    let v = stderr_json(&out);
    assert_eq!(v["type"], "error");
    assert_eq!(v["cli"]["kind"], "usage");
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("Usage:"),
        "must not emit clap human usage text"
    );
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn default_arg_subcommand_conflict_is_json_error() {
    let out = run(&["discover", "http://127.0.0.1:1", "-d", "{}"]);
    assert!(out.stdout.is_empty());
    let v = stderr_json(&out);
    assert_eq!(v["cli"]["kind"], "usage");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn human_unknown_option_stays_human_exit_2() {
    let out = run(&["--human", "--not-a-real-option"]);
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("error:"), "human clap error preserved: {err}");
    assert!(!err.contains("\"type\""), "not JSON under --human");
    assert_eq!(out.status.code(), Some(2));
}

// --- help / version obey the mode ----------------------------------------

#[test]
fn help_and_version_exit_zero_on_stdout() {
    for flag in ["--help", "--version"] {
        let out = run(&[flag]);
        assert!(out.status.success(), "{flag} should exit 0");
        assert!(!out.stdout.is_empty(), "{flag} prints to stdout");
    }
}

#[test]
fn version_default_is_structured_json() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).expect("version -> JSON");
    assert_eq!(v["name"], "rpcurl");
    assert!(v["version"].as_str().unwrap().starts_with("1.1"));
}

#[test]
fn help_default_is_structured_json() {
    let out = run(&["--help"]);
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).expect("help -> JSON");
    assert_eq!(v["name"], "rpcurl");
    assert!(v["version"].as_str().unwrap().starts_with("1.1"));
    assert!(v["usage"].as_str().unwrap().contains("rpcurl"));
    assert!(v["args"].as_array().unwrap().iter().any(|a| a["name"] == "--max-time"),
        "structured args include the flags");
    assert!(v["subcommands"].as_array().unwrap().iter().any(|s| s["name"] == "discover"));
    assert!(v["exit_codes"]["5"].is_string(), "exit-code contract present");
    assert!(v.get("help").is_none(), "no more flat {{\"help\":<text>}} shape");
    // CH1: boolean flags carry no value_name. CH2: aliases are a machine field.
    let args = v["args"].as_array().unwrap();
    let human = args.iter().find(|a| a["name"] == "--human").expect("--human present");
    assert_eq!(human["takes_value"], false);
    assert!(human["value_name"].is_null(), "boolean flag has null value_name: {human}");
    let token = args.iter().find(|a| a["name"] == "--token").expect("--token present");
    assert!(
        token["aliases"].as_array().unwrap().iter().any(|x| x == "--oauth2-bearer"),
        "--oauth2-bearer is a structured alias: {token}"
    );
}

#[test]
fn help_human_mode_is_decorated_text() {
    let out = run(&["--human", "--help"]);
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("Usage:"), "clap decorated help text");
    assert!(!s.trim_start().starts_with('{'), "human help is not JSON");
}

#[test]
fn version_human_mode_is_plain_text() {
    let out = run(&["--human", "--version"]);
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("rpcurl 1.1"));
    assert!(!s.contains('{'), "human version is not JSON");
}

// --- verbose diagnostics (framed JSONL + redaction) ----------------------

#[test]
fn verbose_machine_mode_is_pure_jsonl() {
    // -v + malformed header + connect failure: EVERY stderr line must be JSON.
    let out = run(&[
        "-v", "-H", "noseparator",
        "http://127.0.0.1:1/app/Svc/method", "-d", "{}",
    ]);
    let err = String::from_utf8_lossy(&out.stderr);
    let mut types = Vec::new();
    for line in err.trim().lines() {
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("verbose stderr must be JSONL: {e}; line={line:?}"));
        types.push(v["type"].as_str().unwrap_or("").to_owned());
    }
    assert!(types.iter().any(|t| t == "debug"), "verbose emits debug records: {types:?}");
    assert!(types.iter().any(|t| t == "warning"), "malformed -H warning present: {types:?}");
    assert_eq!(types.last().unwrap(), "error", "terminal error is last: {types:?}");
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn verbose_does_not_leak_bearer_token() {
    let out = run(&[
        "-v", "-t", "SECRET_TOKEN",
        "http://127.0.0.1:1/app/Svc/method", "-d", "{}",
    ]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("SECRET_TOKEN"), "authorization must be redacted: {err}");
    assert!(err.contains("<redacted>"), "redaction marker present");
}

#[test]
fn verbose_does_not_leak_hostname() {
    // The default `c-id` is `r-<hostname>`; verbose must never leak the invoking
    // machine's hostname into any stream (repo rule: no internal hostnames in logs).
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    assert!(!host.is_empty(), "test needs a real hostname");
    let out = run(&["-v", "http://127.0.0.1:1/app/Svc/method", "-d", "{}"]);
    let err = String::from_utf8_lossy(&out.stderr);
    let outv = String::from_utf8_lossy(&out.stdout);
    assert!(!err.contains(&host), "hostname leaked to stderr: {err}");
    assert!(!outv.contains(&host), "hostname leaked to stdout: {outv}");
    assert!(err.contains("c-id: <redacted>"), "c-id present but redacted: {err}");
}

// --- stream split & protocol errors --------------------------------------

#[test]
fn success_puts_data_on_stdout_and_nothing_on_stderr() {
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["discover", &base]);
    assert!(out.status.success());
    assert!(!out.stdout.is_empty(), "payload on stdout");
    assert!(
        out.stderr.is_empty(),
        "no diagnostics on clean success: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn discover_http_404_is_protocol_error_exit_4() {
    let base = spawn_http("404 Not Found", "nope");
    let out = run(&["discover", &base]);
    assert!(out.stdout.is_empty());
    let v = stderr_json(&out);
    assert_eq!(v["cli"]["kind"], "protocol");
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn discover_malformed_body_is_protocol_error_exit_4() {
    let base = spawn_http("200 OK", "this is not json at all");
    let out = run(&["discover", &base]);
    assert!(out.stdout.is_empty());
    assert!(!out.stderr.is_empty());
    assert_eq!(out.status.code(), Some(4));
}
