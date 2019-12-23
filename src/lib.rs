use std::{
    collections::{HashMap, HashSet},
    io::Error,
    thread,
    fmt,
};

use log::*;

use futures::{
    channel::mpsc,
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt
};

use async_std::{
    prelude::*,
    net::{SocketAddr, ToSocketAddrs, TcpListener, TcpStream},
    task,
};

use serde::{Serialize, Deserialize};
use serde_json::json;

use tungstenite;
use tungstenite::protocol::Message as WSMessage;
use async_tungstenite::WebSocketStream;

use zmq;


#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Debug, Hash, Copy)]
struct ClientID(usize);

impl fmt::Display for ClientID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

struct NewConnectionMessage {
    id: ClientID,
    sender: SplitSink<WebSocketStream<TcpStream>, WSMessage>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ActionMessage{
    action: String,
    channel: String,
}


#[derive(Debug)]
struct OutMessage(String);

#[derive(Debug)]
struct WebSocketMessage {
    id: ClientID,
    message: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ZMQMessage {
    channel: String,
    message: String,
}

enum ServerMessage {
    NewConnection(NewConnectionMessage),
    CloseConnection(WebSocketMessage),
    FromWebSocket(WebSocketMessage),
    FromZMQ(ZMQMessage),
}

#[derive(Debug)]
struct Channel {
    name: String,
    clients: HashSet<ClientID>,
}

impl Channel {
    fn add(&mut self, client: ClientID) {
        self.clients.insert(client);
    }
}

struct WebSocketListener {
    id: ClientID,
    addr: SocketAddr,
    listener: SplitStream<WebSocketStream<TcpStream>>,
    sender: mpsc::UnboundedSender<ServerMessage>,
}

pub struct WebSocketServer {
    websocket_addr: String,
    zmq_addr: String,
    senders:
        HashMap<ClientID, SplitSink<WebSocketStream<TcpStream>, WSMessage>>,
    channels: HashMap<String, Channel>,
}

async fn receive_websocket_messages(mut websocket: WebSocketListener) {
    //let mut websocket = websocket;
    while let Some(msg) = websocket.listener.next().await {
        let msg = msg.expect("Failed to get request.");
        if msg.is_binary() || msg.is_text() {
            info!("Received websocket message from {}: {}", websocket.addr, msg);
            websocket
                .sender
                .send(ServerMessage::FromWebSocket(WebSocketMessage {
                    id: websocket.id,
                    message: msg.to_string(),
                })).await
                .unwrap_or_else(|_error|  {
                    warn!("Could not forward websocket message '{}' to server.", msg);
                });
        } else if msg.is_close() {
            websocket.sender.send(ServerMessage::CloseConnection(WebSocketMessage {
                id: websocket.id,
                message: "close".to_string(),
            })).await
                .unwrap_or_else(|_error| {
                    warn!("Could not forward websocket closing message to server.");
                });
        }
    }
}

async fn receive_websocket_connections(
    listener: TcpListener,
    mut server_sender: mpsc::UnboundedSender<ServerMessage>,
) {
    let mut idx = 0;

    while let Ok((stream, _)) = listener.accept().await {
        let addr = stream
            .peer_addr()
            .expect("connected streams should have a peer address");
        let ws_stream = async_tungstenite::accept_async(stream)
            .await
            .expect("Error during the websocket handshake occurred");
        debug!("New WebSocket connection: {}", addr);

        let (sink, stream) = ws_stream.split();
        let client_id = ClientID(idx);

        server_sender.send(ServerMessage::NewConnection(NewConnectionMessage {
            id: client_id,
            sender: sink,
        })).await
            .unwrap_or_else(|_error|  {
                warn!("Could not register new websocket connection with server.");
            });

        // spawn task listeing to sink and dispatch
        task::spawn(receive_websocket_messages(WebSocketListener {
            id: client_id,
            addr,
            listener: stream,
            sender: mpsc::UnboundedSender::clone(&server_sender),
        }));
        idx += 1;
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

    /// Run the server by listening to incoming websocket connections and zeromq messages
    pub async fn run_async(&mut self) -> Result<(), Error> {
        let addr = self
            .websocket_addr
            .to_socket_addrs()
            .await
            .expect("Not a valid address")
            .next()
            .expect("Not a socket address");

        let try_socket = TcpListener::bind(&addr).await;
        let listener = try_socket.expect("Failed to bind");

        info!("Listening for websockets on: {}", self.websocket_addr);
        let (server_sender, mut server_receiver) = mpsc::unbounded();
        task::spawn(receive_websocket_connections(listener, mpsc::UnboundedSender::clone(&server_sender)));

        info!("Listening for zeromq on: {}", self.zmq_addr);
        let context = zmq::Context::new();
        let sub = context.socket(zmq::SUB).unwrap();
        sub.set_subscribe(b"").unwrap();
        sub.bind(&self.zmq_addr)
            .expect("Could not connect to publisher.");

        let mut zmq_sender = mpsc::UnboundedSender::clone(&server_sender);
        thread::spawn(move|| {
            loop {
                let msg = match sub.recv_msg(0) {
                    Ok(x) => x,
                    Err(_err) => {
                        warn!("Error while reading zmq message.");
                        continue;
                    },
                };
                let msg = match std::str::from_utf8(&msg) {
                    Ok(x) => x,
                    Err(_err) => {
                        warn!("Error while parsing zmq message to utf-8.");
                        continue
                    },
                };
                let zmq_msg: ZMQMessage = match serde_json::from_str(&msg) {
                    Ok(x) => x,
                    Err(_err) => {
                        warn!("Error parsing zmq message to json.");
                        continue;
                    }
                };
                info!("Received ZeroMQ Message '{}'", &msg);
                task::block_on(
                async {
                    zmq_sender.send(ServerMessage::FromZMQ(zmq_msg)).await
                        .unwrap_or_else(|_error|  {
                            warn!("Could not forward zmq message '{}' to server.", &msg);
                        });
                });

            }
        });
        
        // dispatch messages from  WebSockets/ZMQ
        while let Some(msg) = server_receiver.next().await {
            match msg {
                ServerMessage::NewConnection(msg) => self.register_websocket(msg.id, msg.sender).await,
                ServerMessage::CloseConnection(msg) => self.unregister_websocket(msg.id).await,
                ServerMessage::FromWebSocket(msg) => self.dispatch(msg).await,
                ServerMessage::FromZMQ(msg) => self.send_channel(&msg.channel, &msg.message).await,
            }
        }

        Ok(())
    }

    /// Store the websocket sink for a given client id
    async fn register_websocket (
        &mut self,
        client_id: ClientID,
        sender: SplitSink<WebSocketStream<TcpStream>, WSMessage>,
    ) {
        debug!("Registering websocket with id '{}'", client_id);
        self.senders.insert(client_id, sender);
    }

    async fn unregister_websocket(&mut self, client_id: ClientID) {
        debug!("Unegistering websocket with id '{}'", client_id);
        self.senders.remove(&client_id);
        for (_, channel) in self.channels.iter_mut() {
            debug!("Removing client '{}' from channel '{}'", client_id, channel.name);
            channel.clients.remove(&client_id);
        }
    }

    /// Dispatch an in coming websocket message by calling the correct method
    /// For now only subscribe is available as action
    async fn dispatch(&mut self, message: WebSocketMessage) {
        
        let msg: ActionMessage = match serde_json::from_str(&message.message) {
            Ok(msg) => msg,
            Err(_err) => {
                warn!("Error parsing websocket message '{}' to json.", message.message);
                return;
            },
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
            .or_insert(Channel {
                name: channel.to_string(),
                clients: HashSet::new(),
            })
            .add(client_id);
        debug!("Channels: {:?}", self.channels);
    }

    /// Send a websocket message to all clients of a channel
    async fn send_channel(&mut self, channel: &str, message: &str) {

        // parse to json
        let message = json!(message);
        
        if let Some(ch) = self.channels.get(channel) {
            debug!("Sending message to channel '{}': {}", channel, message);
            for client_id in ch.clients.iter() {
                if let Some(websocket) = self.senders.get_mut(&client_id) {
                    debug!("Sending channel message to client {}", client_id);
                    websocket.send(WSMessage::Text(message.to_string())).await
                        .unwrap_or_else(|_error|  {
                            warn!("Could send message '{}' to websocket.", message);
                        });
                }
            }
        }
    }
}
