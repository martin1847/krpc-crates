use crate::error::CliError;
use krpc::proto::InputProto;
use serde_json::{from_str, Value};

/// rpcurl — command-line KRPC client.
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

    /// Machine mode: pure JSON on stdout (no emoji/decoration); errors as a JSON
    /// object on stderr. Default human mode keeps the emoji prefixes.
    #[arg(long, global = true)]
    pub(crate) json: bool,

    /// Verbose mode: prints headers, input, URL, etc. (to stderr, never stdout).
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
    /// RPC服务的URL,如 https://demo.krpc.tech/appName/DemoService/methodName
    pub(crate) url: Option<String>,

    /// 入参json, 优先级高于file, e.g. `-d '{"name":"KRPC"}'`
    #[arg(short, long)]
    data: Option<String>,

    /// 入参jsonFile, e.g. `-f test.json`
    #[arg(short, long)]
    file: Option<String>,

    /// Authorization: Bearer <accessToken>, 支持环境变量传值
    #[arg(short, long, env = "KRPC_TOKEN")]
    token: Option<String>,

    /// Cookie, e.g. `tk=j.w.t`
    #[arg(short, long, env = "KRPC_COOKIE")]
    cookie: Option<String>,

    /// 客户端id，便于tracking
    #[arg(short = 'i', long, env = "KRPC_CID")]
    c_id: Option<String>,

    /// 客户端meta
    #[arg(short = 'm', long, env = "KRPC_CMETA")]
    c_meta: Option<String>,

    /// Custom headers, e.g. `-H a=b -H c=d`
    #[arg(short = 'H', long)]
    header: Option<Vec<String>>,
}

const JSON_NULL: &str = "null";

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

    /// Resolve and validate the request JSON body from `-d` / `-f`, defaulting to
    /// JSON `null`. Errors surface as [`CliError::Usage`].
    pub(crate) fn read_data(&self) -> Result<String, CliError> {
        let json_data = if let Some(data) = &self.data {
            data.clone()
        } else if let Some(file) = &self.file {
            std::fs::read_to_string(file)
                .map_err(|e| CliError::Usage(format!("cannot read file < {file} >: {e}")))?
        } else {
            JSON_NULL.to_owned()
        };
        from_str::<Value>(&json_data)
            .map_err(|e| CliError::Usage(format!("failed to parse json < {json_data} >: {e}")))?;
        Ok(json_data)
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
                match kv.split_once('=') {
                    Some((k, v)) => {
                        let key = tonic::metadata::MetadataKey::from_str(k)
                            .map_err(|e| CliError::Usage(format!("bad header key < {k} >: {e}")))?;
                        let val = v
                            .parse()
                            .map_err(|e| CliError::Usage(format!("bad header value < {v} >: {e}")))?;
                        header.insert(key, val);
                    }
                    None => warnings.push(kv.clone()),
                }
            }
        }

        let cid: String = if let Some(c_id) = &self.c_id {
            c_id.clone()
        } else {
            format!("r-{}", gethostname::gethostname().to_str().unwrap_or("unknown"))
        };
        header.insert(
            "c-id",
            cid.parse()
                .map_err(|e| CliError::Usage(format!("bad c-id < {cid} >: {e}")))?,
        );

        if let Some(s) = &self.c_meta {
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
    fn malformed_header_becomes_warning_not_error() {
        let a = InvokeArgs {
            url: Some("http://h/a/S/m".to_owned()),
            header: Some(vec!["noequals".to_owned(), "a=b".to_owned()]),
            ..Default::default()
        };
        let (_req, warnings) = a.build_request("null".to_owned()).unwrap();
        assert_eq!(warnings, vec!["noequals".to_owned()]);
    }
}
