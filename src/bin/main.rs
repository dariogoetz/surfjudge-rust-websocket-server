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

use std::env;
use async_std::task;

use websockets_async::WebSocketServer;


fn main() {
    let _ = env_logger::try_init();
    let websocket_addr = env::args().nth(1).unwrap_or("127.0.0.1:6544".to_string());
    let zmq_addr = env::args().nth(2).unwrap_or("tcp://*:6545".to_string());

    let mut server = WebSocketServer::new(&websocket_addr, &zmq_addr);
    task::block_on(server.run_async()).expect("Error running websocket server.");
}
