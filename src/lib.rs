use std::{
    collections::{HashMap, HashSet},
    fmt,
    io::Error,
    thread,
};

use log::*;

use futures::{
    channel::mpsc,
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};

use async_std::{
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    task,
};

use serde::{Deserialize, Serialize};
use serde_json::json;

use async_tungstenite::WebSocketStream;
use tungstenite;
use tungstenite::protocol::Message;

use zmq;

#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Debug, Hash, Copy)]
struct ClientID(usize);

impl fmt::Display for ClientID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

struct NewConnection {
    id: ClientID,
    sender: SplitSink<WebSocketStream<TcpStream>, Message>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ActionMessage {
    action: String,
    channel: String,
}

#[derive(Debug)]
struct WSMessage {
    id: ClientID,
    message: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ZMQMessage {
    channel: String,
    message: String,
}

enum Event {
    NewConnection(NewConnection),
    CloseConnection(WSMessage),
    FromWebSocket(WSMessage),
    FromZMQ(ZMQMessage),
}

struct WebSocketListener {
    id: ClientID,
    addr: SocketAddr,
    listener: SplitStream<WebSocketStream<TcpStream>>,
    sender: mpsc::UnboundedSender<Event>,
}

pub struct WebSocketServer {
    websocket_addr: String,
    zmq_addr: String,
    senders: HashMap<ClientID, SplitSink<WebSocketStream<TcpStream>, Message>>,
    channels: HashMap<String, HashSet<ClientID>>,
}

async fn receive_websocket_messages(mut websocket: WebSocketListener) {
    while let Some(msg) = websocket.listener.next().await {
        let msg = match msg {
            Ok(x) => x,
            Err(_err) => {
                warn!("Failed to get request.");
                websocket
                    .sender
                    .send(Event::CloseConnection(WSMessage {
                        id: websocket.id,
                        message: "close".to_string(),
                    }))
                    .await
                    .unwrap_or_else(|_error| {
                        warn!("Could not forward websocket closing message to server after ungraceful close.");
                    });
                break;
            },
        };
        if msg.is_binary() || msg.is_text() {
            info!(
                "Received websocket message from {}: {}",
                websocket.addr, msg
            );
            websocket
                .sender
                .send(Event::FromWebSocket(WSMessage {
                    id: websocket.id,
                    message: msg.to_string(),
                }))
                .await
                .unwrap_or_else(|_error| {
                    warn!("Could not forward websocket message '{}' to server.", msg);
                });
        } else if msg.is_close() {
            websocket
                .sender
                .send(Event::CloseConnection(WSMessage {
                    id: websocket.id,
                    message: "close".to_string(),
                }))
                .await
                .unwrap_or_else(|_error| {
                    warn!("Could not forward websocket closing message to server.");
                });
        }
    }
}

async fn receive_websocket_connections(addr: String, mut sender: mpsc::UnboundedSender<Event>) {
    let mut idx = 0;
    let addr = addr
        .to_socket_addrs()
        .await
        .expect("Not a valid address")
        .next()
        .expect("Not a socket address");

    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");

    while let Ok((stream, _)) = listener.accept().await {
        let addr = match stream.peer_addr() {
            Ok(x) => x,
            Err(_err) => {
                warn!("connected streams should have a peer address");
                continue;
            }
        };
        let ws_stream = match async_tungstenite::accept_async(stream).await {
            Ok(x) => x,
            Err(_err) => {
                warn!("Error during the websocket handshake occurred");
                continue;
            },
        };
        debug!("New WebSocket connection: {}", addr);

        let (sink, stream) = ws_stream.split();
        let client_id = ClientID(idx);

        sender
            .send(Event::NewConnection(NewConnection {
                id: client_id,
                sender: sink,
            }))
            .await
            .unwrap_or_else(|_error| {
                warn!("Could not register new websocket connection with server.");
            });

        // spawn task listeing to sink and dispatch
        task::spawn(receive_websocket_messages(WebSocketListener {
            id: client_id,
            addr,
            listener: stream,
            sender: mpsc::UnboundedSender::clone(&sender),
        }));
        idx += 1;
    }
}

fn receive_zmq_messages(addr: String, mut sender: mpsc::UnboundedSender<Event>) {
    let context = zmq::Context::new();
    let sub = context.socket(zmq::SUB).unwrap();
    sub.set_subscribe(b"").unwrap();
    sub.bind(&addr).expect("Could not connect to publisher.");

    loop {
        let msg = match sub.recv_msg(0) {
            Ok(x) => x,
            Err(_err) => {
                warn!("Error while reading zmq message.");
                continue;
            }
        };
        let msg = match std::str::from_utf8(&msg) {
            Ok(x) => x,
            Err(_err) => {
                warn!("Error while parsing zmq message to utf-8.");
                continue;
            }
        };
        let zmq_msg: ZMQMessage = match serde_json::from_str(&msg) {
            Ok(x) => x,
            Err(_err) => {
                warn!("Error parsing zmq message to json.");
                continue;
            }
        };
        info!("Received ZMQ Message '{}'", &msg);
        task::block_on(async {
            sender
                .send(Event::FromZMQ(zmq_msg))
                .await
                .unwrap_or_else(|_error| {
                    warn!("Could not forward zmq message '{}' to server.", &msg);
                });
        });
    }
}

impl WebSocketServer {
    /// Initialize a new WebSocketServer
    pub fn new(websocket_addr: &str, zmq_addr: &str) -> WebSocketServer {
        WebSocketServer {
            websocket_addr: websocket_addr.to_string(),
            zmq_addr: zmq_addr.to_string(),
            senders: HashMap::new(),
            channels: HashMap::new(),
        }
    }

    /// Run the server by listening to incoming websocket connections and ZMQ messages
    pub async fn run_async(&mut self) -> Result<(), Error> {
        let (server_sender, mut server_receiver) = mpsc::unbounded();

        info!("Listening for websockets on: {}", self.websocket_addr);
        let addr = self.websocket_addr.to_string();
        task::spawn(receive_websocket_connections(
            addr,
            mpsc::UnboundedSender::clone(&server_sender),
        ));

        info!("Listening for ZMQ on: {}", self.zmq_addr);
        let addr = self.zmq_addr.to_string();
        thread::spawn(move || {
            receive_zmq_messages(addr, mpsc::UnboundedSender::clone(&server_sender))
        });

        // dispatch messages from  WebSockets/ZMQ
        while let Some(msg) = server_receiver.next().await {
            match msg {
                Event::NewConnection(msg) => self.register_websocket(msg.id, msg.sender).await,
                Event::CloseConnection(msg) => self.unregister_websocket(msg.id).await,
                Event::FromWebSocket(msg) => self.dispatch(msg).await,
                Event::FromZMQ(msg) => self.send_channel(&msg.channel, &msg.message).await,
            }
        }

        Ok(())
    }

    /// Store the websocket sink for a given client id
    async fn register_websocket(
        &mut self,
        client_id: ClientID,
        sender: SplitSink<WebSocketStream<TcpStream>, Message>,
    ) {
        debug!("Registering websocket with id '{}'", client_id);
        self.senders.insert(client_id, sender);
    }

    async fn unregister_websocket(&mut self, client_id: ClientID) {
        debug!("Unegistering websocket with id '{}'", client_id);
        self.senders.remove(&client_id);
        for (channel_name, channel) in self.channels.iter_mut() {
            debug!(
                "Removing client '{}' from channel '{}'",
                client_id, channel_name
            );
            channel.remove(&client_id);
        }
    }

    /// Dispatch an in coming websocket message by calling the correct method
    /// For now only subscribe is available as action
    async fn dispatch(&mut self, message: WSMessage) {
        let msg: ActionMessage = match serde_json::from_str(&message.message) {
            Ok(msg) => msg,
            Err(_err) => {
                warn!(
                    "Error parsing websocket message '{}' to json.",
                    message.message
                );
                return;
            }
        };

        debug!("Dispatching message from WebSocket: {:?}", msg);
        if msg.action == "subscribe" {
            self.subscribe(message.id, &msg.channel).await;
        } else {
            warn!("Unknown action: '{}'", msg.action);
        }
    }

    /// Subscribe a websocket client to a channel
    async fn subscribe(&mut self, client_id: ClientID, channel: &str) {
        debug!("Subscribing '{}' to channel '{}'", client_id, channel);
        self.channels
            .entry(channel.to_string())
            .or_insert_with(HashSet::new)
            .insert(client_id);
        debug!("Channels: {:?}", self.channels);
    }

    /// Send a websocket message to all clients of a channel
    async fn send_channel(&mut self, channel: &str, message: &str) {
        // parse to json
        let message = json!({"channel": channel, "message": message});

        if let Some(ch) = self.channels.get(channel) {
            debug!("Sending message to channel '{}': {}", channel, message);
            for client_id in ch.iter() {
                if let Some(websocket) = self.senders.get_mut(&client_id) {
                    debug!("Sending channel message to client {}", client_id);
                    websocket
                        .send(Message::Text(message.to_string()))
                        .await
                        .unwrap_or_else(|_error| {
                            warn!("Could send message '{}' to websocket.", message);
                        });
                }
            }
        }
    }
}
