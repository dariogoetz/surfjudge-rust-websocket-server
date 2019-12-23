//! A chat server that broadcasts a message to all connections.
//!
//! This is a simple line-based server which accepts WebSocket connections,
//! reads lines from those connections, and broadcasts the lines to all other
//! connected clients.
//!
//! You can test this out by running:
//!
//!     cargo run --example server 127.0.0.1:12345
//!
//! And then in another window run:
//!
//!     cargo run --example client ws://127.0.0.1:12345/
//!
//! You can run the second command in multiple windows and then chat between the
//! two, seeing the messages from the other client as they're received. For all
//! connected clients they'll all join the same room and see everyone else's
//! messages.

use clap::{Arg, App};
use std::env;
use async_std::task;

use websockets_async::WebSocketServer;

fn main() {

    let matches = App::new("Surfjudge WebSocket webserver companion")
        .version("0.1.0")
        .author("Dario Goetz <dario.goetz@googlemail.com>")
        .about("Serves websockets with messages from surfjudge webserver via ZMQ")
        .arg(Arg::with_name("ws hostname")
                 .short("w")
                 .long("websocket-host")
                 .takes_value(true)
                 .help("Hostname of websocket server"))
        .arg(Arg::with_name("ws port")
                 .short("p")
                 .long("websocket-port")
                 .takes_value(true)
                 .help("Port of websocket server"))
        .arg(Arg::with_name("zmq port")
                 .short("z")
                 .long("zeromq-port")
                 .takes_value(true)
             .help("Port of ZeroMQ server"))
    .get_matches();

    let _ = env_logger::try_init();

    let websocket_host = env::var("WEBSOCKETS_HOST")
        .unwrap_or(matches.value_of("ws hostname")
                   .unwrap_or("0.0.0.0").to_string());
    let websocket_port = env::var("WEBSOCKETS_PORT")
        .unwrap_or(matches.value_of("ws port")
                   .unwrap_or("6544").to_string());
    let websocket_addr = format!("{}:{}", websocket_host, websocket_port);

    let zmq_port = env::var("ZMQ_PORT")
        .unwrap_or(matches.value_of("zmq port")
                   .unwrap_or("6545").to_string());
    let zmq_addr = format!("tcp://*:{}", zmq_port);

    let mut server = WebSocketServer::new(&websocket_addr, &zmq_addr);
    task::block_on(server.run_async()).expect("Error running websocket server.");
}
