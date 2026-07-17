//! Introspection: `discover`, `schema`, `example`.
//!
//! Talks plain HTTP to the server's `GET /agent/discover` endpoint (ADR-0004 /
//! AGENT-001 P0), which serves the full [`ApiMeta`] — the same payload that feeds
//! the MCP bridge. From it we derive human/agent-friendly service listings and
//! JSON-Schema-ish method schemas.
//!
//! Network fetch and parsing are separated so schema derivation is unit-testable
//! against a captured/fixture `ApiMeta` with no live server.

use std::collections::BTreeMap;
use std::io::{self, Write};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::Uri;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde::Deserialize;
use serde_json::{json, to_string_pretty, Map, Value};

use crate::error::CliError;
use crate::output::Mode;

// ---------------------------------------------------------------------------
// ApiMeta model (mirror of tech.krpc.common.meta.* as serialized by Jackson).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct ApiMeta {
    pub app: Option<String>,
    #[serde(default)]
    pub apis: Vec<Api>,
    #[serde(default)]
    pub dtos: Vec<Dto>,
    #[serde(rename = "sdkVersion")]
    pub sdk_version: Option<String>,
    #[serde(rename = "apiVersion")]
    pub api_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Api {
    pub name: String,
    #[serde(default)]
    pub methods: Vec<Method>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Method {
    pub name: String,
    pub arg: Option<PropertyType>,
    pub res: Option<PropertyType>,
    #[serde(default)]
    pub annotations: Vec<Anno>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PropertyType {
    #[serde(rename = "rawType")]
    pub raw_type: Dto,
    #[serde(default)]
    pub generics: Vec<PropertyType>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Dto {
    pub name: String,
    #[serde(default)]
    pub fields: Vec<Property>,
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Property {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub prop_type: Option<PropertyType>,
    #[serde(default)]
    pub annotations: Vec<Anno>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Anno {
    pub name: Option<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
}

impl Anno {
    /// Simple annotation name (last `.`-segment): `jakarta...NotBlank` -> `NotBlank`.
    fn simple(&self) -> &str {
        let n = self.name.as_deref().unwrap_or("");
        n.rsplit('.').next().unwrap_or(n)
    }
}

// ---------------------------------------------------------------------------
// Parsing + lookup.
// ---------------------------------------------------------------------------

pub(crate) fn parse_api_meta(body: &str) -> Result<ApiMeta, CliError> {
    serde_json::from_str(body)
        .map_err(|e| CliError::Protocol(format!("could not parse /agent/discover ApiMeta: {e}")))
}

/// All `Service/method` paths in the meta, for did-you-mean suggestions.
fn all_paths(meta: &ApiMeta) -> Vec<String> {
    meta.apis
        .iter()
        .flat_map(|a| a.methods.iter().map(move |m| format!("{}/{}", a.name, m.name)))
        .collect()
}

/// Levenshtein edit distance (full matrix; method inventories are tiny).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Nearest known path within edit distance 2, as a "did you mean" clause (or
/// empty). Enables the agent self-correction loop on a mistyped method.
fn did_you_mean(meta: &ApiMeta, path: &str) -> String {
    let best = all_paths(meta)
        .into_iter()
        .map(|p| (edit_distance(path, &p), p))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d);
    match best {
        Some((_, p)) => format!(" (did you mean `{p}`?)"),
        None => String::new(),
    }
}

/// Locate a method by `Service/method` path within the discovered meta.
fn find_method<'a>(meta: &'a ApiMeta, path: &str) -> Result<(&'a Api, &'a Method), CliError> {
    let (svc, m) = path
        .rsplit_once('/')
        .ok_or_else(|| CliError::Usage(format!("method must be `Service/method`, got < {path} >")))?;
    let api = meta
        .apis
        .iter()
        .find(|a| a.name == svc)
        .ok_or_else(|| CliError::Usage(format!("unknown service < {svc} >{}", did_you_mean(meta, path))))?;
    let method = api
        .methods
        .iter()
        .find(|x| x.name == m)
        .ok_or_else(|| CliError::Usage(format!("unknown method < {path} >{}", did_you_mean(meta, path))))?;
    Ok((api, method))
}

// ---------------------------------------------------------------------------
// Type labels + JSON-schema-ish derivation.
// ---------------------------------------------------------------------------

/// Short generic-aware label, e.g. `List<HelloReply>`.
fn type_label(pt: &PropertyType) -> String {
    let name = &pt.raw_type.name;
    if pt.generics.is_empty() {
        name.clone()
    } else {
        let inner: Vec<String> = pt.generics.iter().map(type_label).collect();
        format!("{name}<{}>", inner.join(", "))
    }
}

fn is_any(name: &str) -> bool {
    matches!(name, "Object" | "Any" | "?")
}

/// Map a `PropertyType` to a JSON-Schema-ish node. `dtos` resolves named complex
/// types to their field definitions; `stack` guards recursive type graphs.
fn type_schema(pt: &PropertyType, dtos: &BTreeMap<String, &Dto>, stack: &mut Vec<String>) -> Value {
    let name = pt.raw_type.name.as_str();
    match name {
        "String" | "CharSequence" | "char" | "Character" => json!({ "type": "string" }),
        "boolean" | "Boolean" => json!({ "type": "boolean" }),
        "byte" | "Byte" | "short" | "Short" | "int" | "Integer" => {
            json!({ "type": "integer", "format": "int32" })
        }
        "long" | "Long" | "BigInteger" => json!({ "type": "integer", "format": "int64" }),
        "float" | "Float" | "double" | "Double" | "BigDecimal" => json!({ "type": "number" }),
        "byte[]" => json!({ "type": "string", "format": "byte" }),
        "void" | "Void" | "null" => json!({ "type": "null" }),
        // RpcResult<T> / Optional<T> are transparent wrappers over the payload.
        "RpcResult" | "Optional" => pt
            .generics
            .first()
            .map(|g| type_schema(g, dtos, stack))
            .unwrap_or_else(|| json!({})),
        "List" | "Set" | "Collection" | "Iterable" | "Array" => {
            let items = pt
                .generics
                .first()
                .map(|g| type_schema(g, dtos, stack))
                .unwrap_or_else(|| json!({}));
            json!({ "type": "array", "items": items })
        }
        "Map" | "HashMap" | "LinkedHashMap" | "TreeMap" => {
            let values = pt
                .generics
                .last()
                .map(|g| type_schema(g, dtos, stack))
                .unwrap_or_else(|| json!({}));
            json!({ "type": "object", "additionalProperties": values })
        }
        _ if is_any(name) => json!({}),
        _ => object_schema(name, &pt.raw_type, dtos, stack),
    }
}

/// Build an object schema from a named DTO. Prefers inline `rawType.fields`,
/// falling back to the top-level `dtos` closure keyed by name.
fn object_schema(
    name: &str,
    inline: &Dto,
    dtos: &BTreeMap<String, &Dto>,
    stack: &mut Vec<String>,
) -> Value {
    if stack.iter().any(|n| n == name) {
        // Recursive reference — stop and name it.
        return json!({ "type": "object", "title": name, "x-ref": name });
    }
    let resolved: &Dto = if !inline.fields.is_empty() {
        inline
    } else if let Some(d) = dtos.get(name) {
        d
    } else {
        // Unknown/opaque complex type: describe by name, no field detail.
        let mut o = Map::new();
        o.insert("type".into(), json!("object"));
        o.insert("title".into(), json!(name));
        return Value::Object(o);
    };

    stack.push(name.to_owned());
    let mut props = Map::new();
    let mut required = Vec::new();
    for f in &resolved.fields {
        let Some(fname) = &f.name else { continue };
        let schema = property_schema(f, dtos, stack);
        if f.annotations.iter().any(|a| {
            matches!(a.simple(), "NotNull" | "NotBlank" | "NotEmpty")
        }) {
            required.push(json!(fname));
        }
        props.insert(fname.clone(), schema);
    }
    stack.pop();

    let mut obj = Map::new();
    obj.insert("type".into(), json!("object"));
    obj.insert("title".into(), json!(name));
    if let Some(doc) = resolved.doc.as_ref().filter(|d| !d.is_empty()) {
        obj.insert("description".into(), json!(doc));
    }
    obj.insert("properties".into(), Value::Object(props));
    if !required.is_empty() {
        obj.insert("required".into(), Value::Array(required));
    }
    Value::Object(obj)
}

/// Schema for a single field: base type schema + validation constraints + @Doc.
fn property_schema(f: &Property, dtos: &BTreeMap<String, &Dto>, stack: &mut Vec<String>) -> Value {
    let mut schema = match &f.prop_type {
        Some(pt) => type_schema(pt, dtos, stack),
        None => json!({}),
    };
    let obj = match schema.as_object_mut() {
        Some(o) => o,
        None => return schema,
    };
    let is_array = obj.get("type").and_then(Value::as_str) == Some("array");
    for a in &f.annotations {
        apply_constraint(obj, a, is_array);
    }
    schema
}

/// Fold one validation annotation / @Doc into a schema object (best-effort;
/// unmapped constraints are preserved under `x-constraints`).
fn apply_constraint(obj: &mut Map<String, Value>, a: &Anno, is_array: bool) {
    let p = &a.properties;
    match a.simple() {
        "Doc" => {
            if let Some(v) = p.get("value").and_then(Value::as_str) {
                obj.insert("description".into(), json!(v));
            }
        }
        "NotBlank" | "NotEmpty" => {
            let key = if is_array { "minItems" } else { "minLength" };
            obj.entry(key.to_owned()).or_insert(json!(1));
        }
        "Size" | "Length" => {
            let (min_k, max_k) = if is_array {
                ("minItems", "maxItems")
            } else {
                ("minLength", "maxLength")
            };
            if let Some(v) = p.get("min") {
                obj.insert(min_k.into(), v.clone());
            }
            if let Some(v) = p.get("max") {
                obj.insert(max_k.into(), v.clone());
            }
        }
        "Min" | "DecimalMin" => {
            if let Some(v) = p.get("value") {
                obj.insert("minimum".into(), coerce_num(v));
            }
        }
        "Max" | "DecimalMax" => {
            if let Some(v) = p.get("value") {
                obj.insert("maximum".into(), coerce_num(v));
            }
        }
        "Positive" => {
            obj.insert("exclusiveMinimum".into(), json!(0));
        }
        "PositiveOrZero" => {
            obj.insert("minimum".into(), json!(0));
        }
        "Negative" => {
            obj.insert("exclusiveMaximum".into(), json!(0));
        }
        "NegativeOrZero" => {
            obj.insert("maximum".into(), json!(0));
        }
        "Pattern" => {
            if let Some(v) = p.get("regexp").or_else(|| p.get("value")).and_then(Value::as_str) {
                obj.insert("pattern".into(), json!(v));
            }
        }
        "Email" => {
            obj.insert("format".into(), json!("email"));
        }
        "NotNull" => {}
        other => {
            // Preserve anything we do not explicitly map so nothing is silently lost.
            let entry = obj
                .entry("x-constraints".to_owned())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(arr) = entry.as_array_mut() {
                arr.push(json!(other));
            }
        }
    }
}

/// Coerce a JSON string like `"10"` to a number where possible (validation
/// annotations often carry numeric bounds as strings).
fn coerce_num(v: &Value) -> Value {
    match v {
        Value::String(s) => s
            .parse::<i64>()
            .map(|n| json!(n))
            .or_else(|_| s.parse::<f64>().map(|n| json!(n)))
            .unwrap_or_else(|_| v.clone()),
        _ => v.clone(),
    }
}

/// Input + output schema for a method, RpcResult-unwrapped on the output side.
fn method_schema(meta: &ApiMeta, method: &Method) -> (Value, Value) {
    let dtos: BTreeMap<String, &Dto> = meta.dtos.iter().map(|d| (d.name.clone(), d)).collect();
    let input = match &method.arg {
        Some(pt) => type_schema(pt, &dtos, &mut Vec::new()),
        None => json!({ "type": "null" }),
    };
    let output = match &method.res {
        Some(pt) => type_schema(pt, &dtos, &mut Vec::new()),
        None => json!({ "type": "null" }),
    };
    (input, output)
}

fn method_doc(method: &Method) -> Option<String> {
    method
        .annotations
        .iter()
        .find(|a| a.simple() == "Doc")
        .and_then(|a| a.properties.get("value"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

// ---------------------------------------------------------------------------
// Example skeleton generation (from a derived schema).
// ---------------------------------------------------------------------------

/// Build a placeholder value matching a schema node. Terminates because
/// [`object_schema`] breaks recursive graphs with an `x-ref` stub (no properties).
pub(crate) fn example_from_schema(schema: &Value) -> Value {
    let Some(obj) = schema.as_object() else {
        return Value::Null;
    };
    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        let mut m = Map::new();
        for (k, v) in props {
            m.insert(k.clone(), example_from_schema(v));
        }
        return Value::Object(m);
    }
    match obj.get("type").and_then(Value::as_str) {
        Some("array") => {
            let item = obj
                .get("items")
                .map(example_from_schema)
                .unwrap_or(Value::Null);
            json!([item])
        }
        Some("object") => Value::Object(Map::new()),
        Some("boolean") => json!(false),
        Some("integer") => json!(0),
        Some("number") => json!(0.0),
        Some("null") => Value::Null,
        Some("string") => {
            if obj.get("format").and_then(Value::as_str) == Some("byte") {
                json!("")
            } else {
                json!("string")
            }
        }
        _ => Value::Null,
    }
}

// ---------------------------------------------------------------------------
// HTTP fetch.
// ---------------------------------------------------------------------------

/// `scheme://authority` of a base url, dropping any path/query.
fn origin_of(base: &str) -> Result<String, CliError> {
    let scheme_end = base
        .find("://")
        .map(|i| i + 3)
        .ok_or_else(|| CliError::Usage(format!("base url must start with http:// or https:// < {base} >")))?;
    let end = base[scheme_end..]
        .find('/')
        .map(|i| scheme_end + i)
        .unwrap_or(base.len());
    Ok(base[..end].to_owned())
}

/// Validate a discover URL scheme; only `http`/`https` are meaningful for the
/// plain-HTTP `/agent/discover` face.
fn web_scheme(uri: &Uri) -> Result<(), CliError> {
    match uri.scheme_str() {
        Some("http") | Some("https") => Ok(()),
        other => Err(CliError::Usage(format!(
            "discover supports http:// and https:// only, got scheme < {} >",
            other.unwrap_or("(none)")
        ))),
    }
}

async fn fetch_api_meta(base_url: &str) -> Result<String, CliError> {
    let url = format!("{}/agent/discover", origin_of(base_url)?);
    let uri: Uri = url
        .parse()
        .map_err(|e| CliError::Usage(format!("invalid discover url < {url} >: {e}")))?;
    web_scheme(&uri)?;

    // rustls + native trust roots — same trust behavior as the gRPC invoke path
    // (krpc's `with_native_roots`), aws-lc-rs provider (already in the tree). The
    // `https_or_http` connector serves both plaintext and TLS origins.
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("aws-lc-rs supports the default protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_or_http()
        .enable_http1()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(https);

    let resp = client
        .get(uri)
        .await
        .map_err(|e| CliError::Connect(format!("GET {url}: {e}")))?;
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .map_err(|e| CliError::Connect(format!("reading {url}: {e}")))?
        .to_bytes();
    if !status.is_success() {
        return Err(CliError::Protocol(format!("GET {url} -> HTTP {status}")));
    }
    String::from_utf8(body.to_vec())
        .map_err(|e| CliError::Protocol(format!("non-utf8 body from {url}: {e}")))
}

// ---------------------------------------------------------------------------
// Rendering.
// ---------------------------------------------------------------------------

fn discover_value(meta: &ApiMeta) -> Value {
    let services: Vec<Value> = meta
        .apis
        .iter()
        .map(|api| {
            let methods: Vec<Value> = api
                .methods
                .iter()
                .map(|m| {
                    json!({
                        "name": m.name,
                        "path": format!("{}/{}", api.name, m.name),
                        "arg": m.arg.as_ref().map(type_label),
                        "res": m.res.as_ref().map(type_label),
                        "doc": method_doc(m),
                    })
                })
                .collect();
            json!({
                "name": api.name,
                "description": api.description,
                "methods": methods,
            })
        })
        .collect();
    json!({
        "app": meta.app,
        "sdkVersion": meta.sdk_version,
        "apiVersion": meta.api_version,
        "services": services,
    })
}

fn render_discover(w: &mut impl Write, mode: Mode, meta: &ApiMeta) -> io::Result<()> {
    match mode {
        Mode::Json => writeln!(w, "{}", to_string_pretty(&discover_value(meta)).unwrap_or_default()),
        Mode::Human => {
            writeln!(
                w,
                "app: {}  (sdk {}, api {})",
                meta.app.as_deref().unwrap_or("?"),
                meta.sdk_version.as_deref().unwrap_or("?"),
                meta.api_version.as_deref().unwrap_or("?"),
            )?;
            for api in &meta.apis {
                let desc = api.description.as_deref().unwrap_or("");
                writeln!(w, "\n{} — {}", api.name, desc)?;
                for m in &api.methods {
                    let arg = m.arg.as_ref().map(type_label).unwrap_or_else(|| "void".into());
                    let res = m.res.as_ref().map(type_label).unwrap_or_else(|| "void".into());
                    let doc = method_doc(m)
                        .map(|d| format!("  // {d}"))
                        .unwrap_or_default();
                    writeln!(w, "  {}/{}({arg}) -> {res}{doc}", api.name, m.name)?;
                }
            }
            Ok(())
        }
    }
}

fn schema_value(meta: &ApiMeta, api: &Api, method: &Method) -> Value {
    let (input, output) = method_schema(meta, method);
    json!({
        "method": format!("{}/{}", api.name, method.name),
        "doc": method_doc(method),
        "input": input,
        "output": output,
    })
}

fn render_schema(
    w: &mut impl Write,
    mode: Mode,
    meta: &ApiMeta,
    api: &Api,
    method: &Method,
) -> io::Result<()> {
    let value = schema_value(meta, api, method);
    let pretty = to_string_pretty(&value).unwrap_or_default();
    match mode {
        Mode::Json => writeln!(w, "{pretty}"),
        Mode::Human => {
            let doc = method_doc(method)
                .map(|d| format!("  // {d}"))
                .unwrap_or_default();
            writeln!(w, "📋 {}/{}{doc}\n{pretty}", api.name, method.name)
        }
    }
}

fn render_example(
    w: &mut impl Write,
    mode: Mode,
    meta: &ApiMeta,
    api: &Api,
    method: &Method,
) -> io::Result<()> {
    let (input, _) = method_schema(meta, method);
    let example = example_from_schema(&input);
    let pretty = to_string_pretty(&example).unwrap_or_default();
    match mode {
        Mode::Json => writeln!(w, "{pretty}"),
        Mode::Human => writeln!(w, "🧪 example input for {}/{}:\n{pretty}", api.name, method.name),
    }
}

// ---------------------------------------------------------------------------
// Command entry points.
// ---------------------------------------------------------------------------

pub(crate) async fn run_discover(w: &mut impl Write, mode: Mode, base_url: &str) -> Result<(), CliError> {
    let meta = parse_api_meta(&fetch_api_meta(base_url).await?)?;
    render_discover(w, mode, &meta).map_err(io_err)
}

pub(crate) async fn run_schema(
    w: &mut impl Write,
    mode: Mode,
    base_url: &str,
    method: &str,
) -> Result<(), CliError> {
    let meta = parse_api_meta(&fetch_api_meta(base_url).await?)?;
    let (api, m) = find_method(&meta, method)?;
    render_schema(w, mode, &meta, api, m).map_err(io_err)
}

pub(crate) async fn run_example(
    w: &mut impl Write,
    mode: Mode,
    base_url: &str,
    method: &str,
) -> Result<(), CliError> {
    let meta = parse_api_meta(&fetch_api_meta(base_url).await?)?;
    let (api, m) = find_method(&meta, method)?;
    render_example(w, mode, &meta, api, m).map_err(io_err)
}

fn io_err(e: io::Error) -> CliError {
    CliError::Usage(format!("write error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/apimeta_quickstart.json");

    fn meta() -> ApiMeta {
        parse_api_meta(FIXTURE).unwrap()
    }

    #[test]
    fn origin_of_keeps_https_scheme() {
        assert_eq!(
            origin_of("https://demo.krpc.tech/quickstart/Hello/hello").unwrap(),
            "https://demo.krpc.tech"
        );
        assert_eq!(origin_of("http://127.0.0.1:50051/app").unwrap(), "http://127.0.0.1:50051");
    }

    #[test]
    fn web_scheme_accepts_http_and_https_only() {
        assert!(web_scheme(&"http://h/agent/discover".parse::<Uri>().unwrap()).is_ok());
        assert!(web_scheme(&"https://h/agent/discover".parse::<Uri>().unwrap()).is_ok());
        let err = web_scheme(&"grpc://h/x".parse::<Uri>().unwrap()).unwrap_err();
        assert_eq!(err.exit_code(), 2, "non-web scheme is a usage error");
    }

    #[test]
    fn parses_fixture() {
        let m = meta();
        assert_eq!(m.app.as_deref(), Some("quickstart"));
        assert_eq!(m.apis.len(), 1);
        assert_eq!(m.apis[0].name, "Hello");
        assert_eq!(m.apis[0].methods[0].name, "hello");
    }

    #[test]
    fn find_method_ok_and_errors() {
        let m = meta();
        assert!(find_method(&m, "Hello/hello").is_ok());
        assert_eq!(find_method(&m, "Hello/nope").unwrap_err().exit_code(), 2);
        assert_eq!(find_method(&m, "Nope/hello").unwrap_err().exit_code(), 2);
        assert_eq!(find_method(&m, "bogus").unwrap_err().exit_code(), 2);
    }

    #[test]
    fn discover_value_lists_methods_with_paths() {
        let v = discover_value(&meta());
        assert_eq!(v["app"], "quickstart");
        let methods = v["services"][0]["methods"].as_array().unwrap();
        assert_eq!(methods[0]["path"], "Hello/hello");
        assert_eq!(methods[0]["arg"], "HelloRequest");
        assert_eq!(methods[0]["res"], "RpcResult<HelloReply>");
        assert_eq!(methods[0]["doc"], "Returns a greeting for the given name.");
    }

    #[test]
    fn schema_input_has_required_notblank_field() {
        let m = meta();
        let (api, method) = find_method(&m, "Hello/hello").unwrap();
        let v = schema_value(&m, api, method);
        let input = &v["input"];
        assert_eq!(input["type"], "object");
        assert_eq!(input["title"], "HelloRequest");
        assert_eq!(input["properties"]["name"]["type"], "string");
        // @NotBlank -> required + minLength 1
        assert_eq!(input["properties"]["name"]["minLength"], 1);
        assert_eq!(input["required"], json!(["name"]));
    }

    #[test]
    fn schema_output_unwraps_rpcresult() {
        let m = meta();
        let (api, method) = find_method(&m, "Hello/hello").unwrap();
        let v = schema_value(&m, api, method);
        let output = &v["output"];
        // RpcResult<HelloReply> collapses to the HelloReply object.
        assert_eq!(output["type"], "object");
        assert_eq!(output["title"], "HelloReply");
        assert_eq!(output["properties"]["message"]["type"], "string");
        assert_eq!(output["properties"]["timestamp"]["format"], "int64");
    }

    #[test]
    fn example_matches_input_shape() {
        let m = meta();
        let (_api, method) = find_method(&m, "Hello/hello").unwrap();
        let (input, _) = method_schema(&m, method);
        let ex = example_from_schema(&input);
        assert_eq!(ex, json!({ "name": "string" }));
    }

    #[test]
    fn origin_strips_path() {
        assert_eq!(origin_of("http://h:8080/app/S/m").unwrap(), "http://h:8080");
        assert_eq!(origin_of("http://h:8080").unwrap(), "http://h:8080");
        assert_eq!(origin_of("no-scheme").unwrap_err().exit_code(), 2);
    }
}
