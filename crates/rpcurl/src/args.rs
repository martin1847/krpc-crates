use krpc::proto::InputProto;
use serde_json::{from_str, Value};

#[derive(clap::Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub(crate) struct Args {
    /// RPC服务的URL,如 https://demo.krpc.tech/appName/DemoService/methodName
    url: String,

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

    // same with python
    /// Verbose mode, prints headers, input, URL, etc.
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    pub verbose: bool,
}

const JSON_NULL: &str = "null";

const HTTP_PREFIX_SIZE: usize = "https://".len();

// without env feauture , customer .
impl Args {
    // fn from_env() -> Self {
    //     let mut args = Args::parse();

    //     // Handle environment variables if command line arguments are not provided
    //     // args.token = args.token.or_else(|| env::var("RPC_TOKEN").ok());
    //     args.cookie = args.cookie.or_else(|| env::var("RPC_COOKIE").ok());
    //     args.c_id = args.c_id.or_else(|| env::var("RPC_CID").ok());
    //     args.c_meta = args.c_meta.or_else(|| env::var("RPC_CMETA").ok());

    //     args
    // }

    pub(crate) fn parse_url(&self) -> (String, String) {
        let url = &self.url;
        if self.verbose {
            println!("[krpc url]:\n{}\n", url);
        }

        let path_slash = HTTP_PREFIX_SIZE + &url[HTTP_PREFIX_SIZE..].find('/').unwrap();

        let http_host = url[0..path_slash].to_string();
        let method_path = url[path_slash..].to_string();
        (http_host, method_path)
    }

    pub(crate) fn to_req(&self) -> tonic::Request<InputProto> {
        let json = self.parse_data();

        let mut req = tonic::Request::new(InputProto { json });

        self.fill_headers(req.metadata_mut());

        req
    }

    fn fill_headers(&self, header: &mut tonic::metadata::MetadataMap) {
        self.header.as_ref().map(|list: &Vec<String>|
            // parse_customer_headers(&list,
            //     |k,v|{header.insert(k.as_str(), v.parse().unwrap());})
            for kv in list{
                let parts: Vec<&str> = kv.split('=').collect();
                if parts.len() == 2 {
                    use std::str::FromStr;
                    let a = tonic::metadata::MetadataKey::from_str(parts[0]).unwrap();
                    let b = parts[1];
                    header.insert(a,b.parse().unwrap());
                } else {
                    println!("ignore malformed -H [ {} ]",kv);
                }
            }
        );

        let cid: String = if let Some(c_id) = &self.c_id {
            c_id.to_owned()
        } else {
            format!("r-{}", gethostname::gethostname().to_str().unwrap())
        };

        header.insert("c-id", cid.parse().unwrap());

        self.c_meta
            .as_ref()
            .map(|s| header.insert("c-meta", s.parse().unwrap()));
        self.token
            .as_ref()
            .map(|s| header.insert("authorization", format!("Bearer {}", s).parse().unwrap()));
        self.cookie
            .as_ref()
            .map(|s| header.insert("cookie", s.parse().unwrap()));

        if self.verbose {
            println!("[request headers]:\n{:?}\n", header)
        }
    }

    fn parse_data(&self) -> String {
        let json_data = if let Some(data) = &self.data {
            data.to_string()
        } else if let Some(file) = &self.file {
            use std::fs;
            // use std::str::FromStr;
            fs::read_to_string(file).unwrap()
        } else {
            //JSON null
            JSON_NULL.to_string()
        };
        let _check_input = from_str::<Value>(&json_data)
            .expect(format!("Falied to Parse Json < {} >", &json_data).as_str());
        if self.verbose {
            println!("[request json]:\n{}\n", json_data);
        }
        // println!("{:?}", args);
        json_data
    }
}
