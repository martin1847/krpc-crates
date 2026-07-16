//! Output routing: success data always to stdout, errors always to stderr.
//!
//! Rendering never uses `print!`/`eprintln!` — every byte goes through an
//! explicit [`Write`] handle, which keeps the streams injectable for tests and
//! guarantees the stdout/stderr split holds under piping.

use std::io::{self, Write};

use krpc::proto::{Out, OutputProto};
use serde_json::{from_str, json, to_string_pretty, Value};

use crate::error::CliError;

/// Presentation mode selected by `--json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Human: emoji-prefixed pretty output.
    Human,
    /// Machine: pure JSON, no emoji or decoration.
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
/// normalized `data` array, which is `--json` only).
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

/// Render an error to `w` (stderr). Human mode prefixes ❌; JSON mode emits a
/// single-line `{"error":{...}}` object. NEVER writes to stdout.
pub(crate) fn render_error(w: &mut impl Write, mode: Mode, err: &CliError) -> io::Result<()> {
    match mode {
        Mode::Human => match err.remote_code() {
            Some(code) => writeln!(w, "❌ [code {code}] {}", err.message()),
            None => writeln!(w, "❌ {}", err.message()),
        },
        Mode::Json => {
            let mut obj = json!({
                "error": {
                    "kind": err.kind(),
                    "msg": err.message(),
                }
            });
            if let Some(code) = err.remote_code() {
                obj["error"]["code"] = json!(code);
            }
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
        // The entire stdout payload must parse as JSON.
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
        // Byte-identical to pre-r1 `main.rs`: emoji + hand-formatted `data<bytes>`.
        assert_eq!(s, "🌱\n{\n    \"code\":0,\n    \"data<bytes>\":[1, 2, 3]\n}\n");
    }

    #[test]
    fn error_json_mode_is_object_with_kind() {
        let err = CliError::Remote {
            code: 3,
            msg: "boom".to_owned(),
        };
        let s = err_to_string(Mode::Json, &err);
        assert!(!s.contains('❌'));
        let v: Value = from_str(s.trim()).unwrap();
        assert_eq!(v["error"]["kind"], "remote");
        assert_eq!(v["error"]["code"], 3);
        assert_eq!(v["error"]["msg"], "boom");
    }

    #[test]
    fn error_human_mode_uses_cross() {
        let err = CliError::Connect("refused".to_owned());
        let s = err_to_string(Mode::Human, &err);
        assert!(s.starts_with('❌'));
        assert!(s.contains("refused"));
    }
}
