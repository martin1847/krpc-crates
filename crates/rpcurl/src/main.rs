mod args;
use clap::Parser;
use krpc::{
    clt::KrpcClient,
    proto::{Out, OutputProto},
};
use serde_json::{from_str, to_string_pretty, Value};
// use krpc::proto::InputProto;

// use reqwest::Client;
// use serde::{Deserialize, Serialize};
// use serde_json::{from_str,Value};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = args::Args::parse();

    let (http_host, method_path) = args.parse_url();

    let mut client = KrpcClient::connect(http_host).await?;
    // let mut client =  KrpcClient::connect("http://127.0.0.1:50051").await?;

    //应用级直接在这里抛出了 Error: Status { code: InvalidArgument,
    let response = client.call(&method_path, args.to_req()).await?;

    if args.verbose {
        println!("[response headers]:\n{:?}\n", response.metadata());
        let ext = response.extensions();
        if ! ext.is_empty() {
            println!("[response extensions]:\n{:?}\n", ext);
        }
    }

    print_output_proto(response.get_ref(),args.verbose);

    // println!("data:\n\n{:?}\n\n", );
    // let bs:Vec<u8> = vec![12,250,168,112];
    // println!("🌱\n{{\n    \"code\":0,\n    \"data\":{:?}\n}}",bs);

    Ok(())
}

fn print_output_proto(res: &OutputProto,verbose:bool) {
    let code = res.code;
    let out = res.out.as_ref().unwrap();
    match out {
        Out::Error(msg) => {
            println!("❌\n{{\"code\":{code},\"msg\":\"{msg}\"}}");
        }
        Out::Json(json) => {
            if verbose {
                println!("[response raw json]:\n{}\n", json);
            }
            let jobj = from_str::<Value>(&format!("{{\"code\":{code},\"data\":{json}}}")).unwrap();
            println!("🍀\n{}",to_string_pretty(&jobj).unwrap());
        }
        Out::Bytes(bs) => {
            println!("🌱\n{{\n    \"code\":{code},\n    \"data<bytes>\":{bs:?}\n}}");
        }
    }
}
