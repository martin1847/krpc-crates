//! End-to-end CLI tests: run the compiled `rpcurl` binary and assert the
//! stdout/stderr split and exit codes hold under real process piping.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::thread;

const BIN: &str = env!("CARGO_BIN_EXE_rpcurl");
const FIXTURE: &str = include_str!("fixtures/apimeta_quickstart.json");

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("spawn rpcurl")
}

/// Serve a fixed HTTP `status` line (e.g. `"200 OK"`) and `body` for any request
/// from a throwaway localhost server. Returns `http://127.0.0.1:<port>`.
fn spawn_http(status: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // Drain request headers up to the blank line.
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

/// Throwaway server that serves the fixture ApiMeta with HTTP 200.
fn spawn_discover_server(body: &'static str) -> String {
    spawn_http("200 OK", body)
}

// --- error routing -------------------------------------------------------

#[test]
fn connect_error_goes_to_stderr_not_stdout() {
    // Unreachable endpoint: transport error must land on fd2 with empty stdout.
    let out = run(&["http://127.0.0.1:1/app/Svc/method", "-d", "{}"]);
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty on error, got: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !out.stderr.is_empty(),
        "stderr must carry the error message"
    );
    assert_eq!(out.status.code(), Some(3), "connect error -> exit 3");
}

#[test]
fn json_mode_error_is_json_object_on_stderr() {
    let out = run(&["--json", "http://127.0.0.1:1/app/Svc/method", "-d", "{}"]);
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    let v: serde_json::Value = serde_json::from_str(err.trim()).expect("stderr must be JSON");
    assert_eq!(v["error"]["kind"], "connect");
    assert!(!err.contains('❌'), "json mode error must not be decorated");
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn bad_url_is_usage_error_exit_2() {
    let out = run(&["not-a-url", "-d", "{}"]);
    assert!(out.stdout.is_empty());
    assert!(!out.stderr.is_empty());
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn bad_json_input_is_usage_error_exit_2() {
    let out = run(&["http://127.0.0.1:1/app/Svc/method", "-d", "{bad"]);
    assert!(out.stdout.is_empty());
    assert_eq!(out.status.code(), Some(2));
}

// --- discover / schema over a real HTTP round-trip -----------------------

#[test]
fn discover_json_is_pure_json_on_stdout() {
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["discover", &base, "--json"]);
    assert!(out.status.success(), "discover should succeed: {:?}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(!s.contains('🍀') && !s.contains('❌'));
    let v: serde_json::Value = serde_json::from_str(s.trim()).expect("stdout must be pure JSON");
    assert_eq!(v["app"], "quickstart");
    assert_eq!(v["services"][0]["methods"][0]["path"], "Hello/hello");
}

#[test]
fn discover_human_lists_methods() {
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["discover", &base]);
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("Hello/hello("), "human listing: {s}");
}

#[test]
fn schema_json_has_input_and_output() {
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["schema", &base, "Hello/hello", "--json"]);
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
fn example_produces_skeleton_json() {
    let base = spawn_discover_server(FIXTURE);
    let out = run(&["example", &base, "Hello/hello", "--json"]);
    assert!(out.status.success());
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(v, serde_json::json!({ "name": "string" }));
}

// --- r2: JSON-mode clap parse errors ------------------------------------

#[test]
fn json_mode_unknown_option_is_json_error_on_stderr() {
    let out = run(&["--json", "--not-a-real-option"]);
    assert!(out.stdout.is_empty(), "clap errors must not touch stdout");
    let err = String::from_utf8_lossy(&out.stderr);
    let v: serde_json::Value =
        serde_json::from_str(err.trim()).expect("json-mode parse error must be a JSON object");
    assert_eq!(v["error"]["kind"], "usage");
    assert!(!err.contains("Usage:"), "must not emit clap human usage text: {err}");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn json_mode_arg_subcommand_conflict_is_json_error() {
    // A subcommand plus invoke args conflict (args_conflicts_with_subcommands).
    let out = run(&["--json", "discover", "http://127.0.0.1:1", "-d", "{}"]);
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    let v: serde_json::Value =
        serde_json::from_str(err.trim()).expect("conflict must render as JSON");
    assert_eq!(v["error"]["kind"], "usage");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn non_json_unknown_option_stays_human_exit_2() {
    let out = run(&["--not-a-real-option"]);
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("error:"), "human clap error preserved: {err}");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn help_and_version_exit_zero_on_stdout() {
    for flag in ["--help", "--version"] {
        let out = run(&[flag]);
        assert!(out.status.success(), "{flag} should exit 0");
        assert!(!out.stdout.is_empty(), "{flag} prints to stdout");
    }
    // --json must not turn help into a JSON error.
    let out = run(&["--json", "--help"]);
    assert!(out.status.success());
    assert!(!out.stdout.is_empty());
}

// --- r2: reached-but-misbehaving server -> protocol error (exit 4) -------

#[test]
fn discover_http_404_is_protocol_error_exit_4() {
    let base = spawn_http("404 Not Found", "nope");
    let out = run(&["discover", &base, "--json"]);
    assert!(out.stdout.is_empty());
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stderr).trim()).unwrap();
    assert_eq!(v["error"]["kind"], "protocol");
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
