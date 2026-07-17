//! Output routing: success data always to stdout, errors/diagnostics always to
//! stderr.
//!
//! Rendering never uses `print!`/`eprintln!` — every byte goes through an
//! explicit [`Write`] handle, which keeps the streams injectable for tests and
//! guarantees the stdout/stderr split holds under piping.
//!
//! In machine ([`Mode::Json`]) mode, stderr is a **JSONL** diagnostics stream:
//! exactly one JSON object per line, each with a `type` discriminant
//! (`"warning"` | `"error"`), so an agent can tell a warning from the terminal
//! error. The terminal error carries the AGENT-002 triplet
//! `{code, message, violations?}` at the top level plus the reserved framing
//! keys `type` and `cli` — nothing else. `violations` is omitted until the
//! server publishes a versioned Status-detail schema (graceful degradation).

use std::io::{self, Write};

use krpc::proto::{Out, OutputProto};
use serde_json::{from_str, json, to_string_pretty, Value};

use crate::error::CliError;

/// Presentation mode, selected solely by the `--human` flag (absent = machine
/// JSON, the default). No TTY detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Human: emoji-prefixed pretty output, decorated stderr diagnostics.
    Human,
    /// Machine: pure JSON payload on stdout, JSONL diagnostics on stderr.
    Json,
}

/// Build the `{"code":..,"data":..}` success envelope as a JSON value.
fn success_value(res: &OutputProto) -> Value {
    let code = res.code;
    match &res.out {
        Some(Out::Json(json_str)) => {
            // Server payload is a raw JSON document; embed it structurally.
            let data = from_str::<Value>(json_str).unwrap_or_else(|_| Value::String(json_str.clone()));
            json!({ "code": code, "data": data })
        }
        Some(Out::Bytes(bs)) => json!({ "code": code, "data": bs }),
        // Out::Error is converted to CliError by the caller before we get here;
        // None is a degenerate empty response. Represent both as null data.
        Some(Out::Error(msg)) => json!({ "code": code, "error": msg }),
        None => json!({ "code": code, "data": Value::Null }),
    }
}

/// Emoji prefix for a successful response in human mode.
fn success_emoji(res: &OutputProto) -> &'static str {
    match &res.out {
        Some(Out::Bytes(_)) => "🌱",
        _ => "🍀",
    }
}

/// Render a successful response to `w` (stdout). In [`Mode::Json`] the output is
/// pure JSON with no emoji. In [`Mode::Human`] the output is byte-for-byte the
/// historical format: an emoji prefix line then the envelope — and for
/// `Out::Bytes`, the original hand-formatted `data<bytes>` shape (not the
/// normalized `data` array, which is machine-mode only).
pub(crate) fn render_success(
    w: &mut impl Write,
    mode: Mode,
    res: &OutputProto,
) -> io::Result<()> {
    if let (Mode::Human, Some(Out::Bytes(bs))) = (mode, &res.out) {
        // Preserve the exact pre-r1 default output for byte payloads.
        return writeln!(
            w,
            "🌱\n{{\n    \"code\":{},\n    \"data<bytes>\":{:?}\n}}",
            res.code, bs
        );
    }
    let value = success_value(res);
    let pretty = to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    match mode {
        Mode::Human => writeln!(w, "{}\n{}", success_emoji(res), pretty),
        Mode::Json => writeln!(w, "{pretty}"),
    }
}

/// Render the terminal error to `w` (stderr). Human mode prefixes ❌ (and ↳ for
/// a hint). Machine mode emits a single-line JSONL record framed with
/// `type:"error"`. NEVER writes to stdout.
pub(crate) fn render_error(w: &mut impl Write, mode: Mode, err: &CliError) -> io::Result<()> {
    match mode {
        Mode::Human => {
            match err.remote_code() {
                Some(code) => writeln!(w, "❌ [code {code}] {}", err.message())?,
                None => writeln!(w, "❌ {}", err.message())?,
            }
            if let Some(hint) = err.hint() {
                writeln!(w, "   ↳ {hint}")?;
            }
            Ok(())
        }
        Mode::Json => {
            // Top level = AGENT-002 triplet {code, message, violations?}.
            // `violations` is omitted (graceful degradation) until the server
            // ships a versioned Status-detail schema. CLI-only fields are
            // namespaced under `cli`; `type` frames the diagnostics stream.
            let mut cli = json!({ "kind": err.kind(), "exit": err.exit_code() });
            if let Some(code) = err.remote_code() {
                cli["grpcCode"] = json!(code);
            }
            if let Some(hint) = err.hint() {
                cli["hint"] = json!(hint);
            }
            let obj = json!({
                "type": "error",
                "code": err.code_str(),
                "message": err.message(),
                "cli": cli,
            });
            writeln!(w, "{obj}")
        }
    }
}

/// Render a non-fatal warning to `w` (stderr). Human mode prefixes ⚠️; machine
/// mode emits a single-line JSONL record framed with `type:"warning"` and a
/// stable `code`. NEVER writes to stdout.
pub(crate) fn render_warning(
    w: &mut impl Write,
    mode: Mode,
    code: &str,
    message: &str,
) -> io::Result<()> {
    match mode {
        Mode::Human => writeln!(w, "⚠️ {message}"),
        Mode::Json => {
            let obj = json!({ "type": "warning", "code": code, "message": message });
            writeln!(w, "{obj}")
        }
    }
}

/// Render a verbose debug line to `w` (stderr). Human mode is a labeled block;
/// machine mode emits a framed JSONL record (`type:"debug"`) so `-v` never
/// corrupts the JSONL diagnostics stream. NEVER writes to stdout.
pub(crate) fn render_debug(
    w: &mut impl Write,
    mode: Mode,
    label: &str,
    body: &str,
) -> io::Result<()> {
    match mode {
        Mode::Human => writeln!(w, "[{label}]:\n{body}\n"),
        Mode::Json => {
            let obj = json!({ "type": "debug", "label": label, "message": body });
            writeln!(w, "{obj}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_to_string(mode: Mode, res: &OutputProto) -> String {
        let mut buf = Vec::new();
        render_success(&mut buf, mode, res).unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn err_to_string(mode: Mode, err: &CliError) -> String {
        let mut buf = Vec::new();
        render_error(&mut buf, mode, err).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn json_mode_success_is_pure_json_no_emoji() {
        let res = OutputProto {
            code: 0,
            out: Some(Out::Json(r#"{"message":"hi"}"#.to_owned())),
        };
        let s = render_to_string(Mode::Json, &res);
        assert!(!s.contains('🍀'), "json mode must not contain emoji: {s:?}");
        assert!(!s.contains('❌'));
        let v: Value = from_str(s.trim()).expect("json mode output must be valid JSON");
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["message"], "hi");
    }

    #[test]
    fn human_mode_success_keeps_emoji() {
        let res = OutputProto {
            code: 0,
            out: Some(Out::Json(r#"{"a":1}"#.to_owned())),
        };
        let s = render_to_string(Mode::Human, &res);
        assert!(s.starts_with('🍀'), "human mode keeps the lucky clover: {s:?}");
    }

    #[test]
    fn bytes_json_mode_is_pure_json() {
        let res = OutputProto {
            code: 0,
            out: Some(Out::Bytes(vec![1, 2, 3])),
        };
        let s = render_to_string(Mode::Json, &res);
        assert!(!s.contains('🌱'));
        let v: Value = from_str(s.trim()).unwrap();
        assert_eq!(v["data"], json!([1, 2, 3]));
    }

    #[test]
    fn bytes_human_mode_matches_historical_format() {
        let res = OutputProto {
            code: 0,
            out: Some(Out::Bytes(vec![1, 2, 3])),
        };
        let s = render_to_string(Mode::Human, &res);
        assert_eq!(s, "🌱\n{\n    \"code\":0,\n    \"data<bytes>\":[1, 2, 3]\n}\n");
    }

    // RPCURL-002: machine error record is the AGENT-002 triplet + `type` + `cli`.
    // (Replaces RPCURL-001's `{"error":{"kind","msg","code"}}` assertion.)
    #[test]
    fn error_json_mode_is_agent002_envelope() {
        let err = CliError::Remote {
            code: 3,
            msg: "boom".to_owned(),
            hint: Some("rpcurl schema http://h Svc/m".to_owned()),
        };
        let s = err_to_string(Mode::Json, &err);
        assert!(!s.contains('❌'));
        let v: Value = from_str(s.trim()).expect("machine error must be one JSON object");
        // Top-level triplet (violations omitted until the server schema ships).
        assert_eq!(v["type"], "error");
        assert_eq!(v["code"], "INVALID_ARGUMENT");
        assert_eq!(v["message"], "boom");
        assert!(v.get("violations").is_none(), "violations omitted, not null");
        assert!(v.get("msg").is_none(), "legacy `msg` key is gone");
        // CLI-only fields are namespaced.
        assert_eq!(v["cli"]["kind"], "remote");
        assert_eq!(v["cli"]["exit"], 1);
        assert_eq!(v["cli"]["grpcCode"], 3);
        assert_eq!(v["cli"]["hint"], "rpcurl schema http://h Svc/m");
    }

    #[test]
    fn error_code_str_maps_cli_kinds() {
        let s = err_to_string(Mode::Json, &CliError::Connect("x".to_owned()));
        let v: Value = from_str(s.trim()).unwrap();
        assert_eq!(v["code"], "CONNECT");
        assert_eq!(v["cli"]["kind"], "connect");
        assert!(v["cli"].get("grpcCode").is_none());
    }

    #[test]
    fn timeout_error_is_exit_5() {
        let s = err_to_string(Mode::Json, &CliError::Timeout("deadline".to_owned()));
        let v: Value = from_str(s.trim()).unwrap();
        assert_eq!(v["code"], "TIMEOUT");
        assert_eq!(v["cli"]["exit"], 5);
    }

    #[test]
    fn error_human_mode_uses_cross() {
        let err = CliError::Connect("refused".to_owned());
        let s = err_to_string(Mode::Human, &err);
        assert!(s.starts_with('❌'));
        assert!(s.contains("refused"));
    }

    #[test]
    fn warning_json_mode_is_framed_record() {
        let mut buf = Vec::new();
        render_warning(&mut buf, Mode::Json, "W_HEADER_MALFORMED", "ignoring -H 'x'").unwrap();
        let v: Value = from_str(String::from_utf8(buf).unwrap().trim()).unwrap();
        assert_eq!(v["type"], "warning");
        assert_eq!(v["code"], "W_HEADER_MALFORMED");
    }

    #[test]
    fn warning_human_mode_is_decorated() {
        let mut buf = Vec::new();
        render_warning(&mut buf, Mode::Human, "W_HEADER_MALFORMED", "ignoring -H 'x'").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with('⚠'));
        assert!(!s.contains("\"type\""));
    }

    #[test]
    fn debug_json_mode_is_framed_record() {
        let mut buf = Vec::new();
        render_debug(&mut buf, Mode::Json, "krpc url", "http://h/a/S/m").unwrap();
        let v: Value = from_str(String::from_utf8(buf).unwrap().trim()).unwrap();
        assert_eq!(v["type"], "debug");
        assert_eq!(v["label"], "krpc url");
        assert_eq!(v["message"], "http://h/a/S/m");
    }

    #[test]
    fn debug_human_mode_is_labeled_block() {
        let mut buf = Vec::new();
        render_debug(&mut buf, Mode::Human, "krpc url", "http://h").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("[krpc url]:"));
        assert!(!s.contains("\"type\""));
    }
}
