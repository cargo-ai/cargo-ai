use reqwest;
use futures::future::join_all;
mod args;

use serde::{Deserialize, Serialize};
use jsonlogic::apply;

fn main() {

    let cmd_args = args::build_cli();

    // Begin: Argument assignments

    let mut server = String::new();
    if let Some(server_arg) = cmd_args.get_one::<String>("server") {
        server.push_str(&server_arg.to_lowercase());
    }

    let mut token = String::new();
    if let Some(cmd_token) = cmd_args.get_one::<String>("token") {
        token.push_str(cmd_token);
    }

    let mut model = String::new();
    if let Some(model_arg) = cmd_args.get_one::<String>("model") {
        model.push_str(model_arg);
    }

    // cmd_args timeout_in_sec default to 60
    let timeout_in_sec = cmd_args
        .get_one::<String>("timeout_in_sec")
        .expect("Timeout value expected")
        .parse::<u64>()
        .expect("Expected unsigned int, u64");


    // End: Argument assignments

    println!("Hello, world!");
}
