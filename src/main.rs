use async_std::task;
use clap::{App, Arg};
use std::env;
use dotenv;


use log::*;

use websockets_async::WebSocketServer;

fn main() {
    let matches = App::new("Surfjudge WebSocket webserver companion")
        .version("0.1.0")
        .author("Dario Goetz <dario.goetz@googlemail.com>")
        .about("Serves websockets with messages from surfjudge webserver via ZMQ")
        .arg(
            Arg::with_name("ws hostname")
                .short("w")
                .long("websocket-host")
                .default_value("0.0.0.0")
                .value_name("hostname")
                .help("Hostname of websocket server"),
        )
        .arg(
            Arg::with_name("ws port")
                .short("p")
                .long("websocket-port")
                .default_value("6544")
                .value_name("port")
                .help("Port of websocket server"),
        )
        .arg(
            Arg::with_name("zmq port")
                .short("z")
                .long("zeromq-port")
                .value_name("port")
                .default_value("6545")
                .help("Port of ZeroMQ server"),
        )
        .get_matches();

    // load .env file from directory
    dotenv::dotenv().ok();

    let _ = env_logger::try_init();

    let websocket_host =
        env::var("WEBSOCKETS_HOST").unwrap_or(matches.value_of("ws hostname").unwrap().to_string());
    let websocket_port =
        env::var("WEBSOCKETS_PORT").unwrap_or(matches.value_of("ws port").unwrap().to_string());
    let websocket_addr = format!("{}:{}", websocket_host, websocket_port);

    let zmq_port =
        env::var("ZMQ_PORT").unwrap_or(matches.value_of("zmq port").unwrap().to_string());
    let zmq_addr = format!("tcp://*:{}", zmq_port);

    let mut server = WebSocketServer::new(&websocket_addr, &zmq_addr);
    info!("Starting websocket server at {} connecting to ZMQ at {}", websocket_addr, zmq_addr);
    task::block_on(server.run_async()).expect("Error running websocket server.");
}
