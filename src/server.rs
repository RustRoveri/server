//! Implements the `Server` struct for managing server's operations.

use crate::assembler::{AssemblerStatus, InsertFragmentError, RetrieveError};
use crate::assemblers_manager::AssemblersManager;
use crate::chat_behavior::ChatBehavior;
use crate::fragment_manager::{FragmentManager, ToBeSentFragment};
use crate::fragmenter::Fragmenter;
use crate::media_behavior::MediaBehavior;
use crate::specialized_behavior::{SetPathError, SpecializedBehavior};
use crate::text_behavior::TextBehavior;
use crate::topology::{RoutingError, Topology};
use crossbeam_channel::{select_biased, Receiver, Sender};
use log::{error, info, warn};
use rust_roveri_api::{FloodId, ServerCommand, ServerEvent, ServerType, SessionId};
use std::collections::HashMap;
use std::path::PathBuf;
use wg_2024::network::SourceRoutingHeader;
use wg_2024::packet::{Ack, FloodRequest, FloodResponse, Nack, NackType, NodeType};
use wg_2024::{
    network::NodeId,
    packet::{Fragment, Packet, PacketType},
};

pub struct Server {
    id: NodeId,
    command_recv: Receiver<ServerCommand>,
    packet_recv: Receiver<Packet>,
    controller_send: Sender<ServerEvent>,
    packet_send: HashMap<NodeId, Sender<Packet>>,
    fragmenter: Fragmenter,
    assemblers_manager: AssemblersManager,
    fragment_manager: FragmentManager,
    topology: Topology,
    specialized: Box<dyn SpecializedBehavior + Send>,
    should_terminate: bool,
    flood_id: FloodId,
}

impl Server {
    pub fn new(
        id: NodeId,
        command_recv: Receiver<ServerCommand>,
        packet_recv: Receiver<Packet>,
        controller_send: Sender<ServerEvent>,
        server_type: ServerType,
    ) -> Self {
        Self {
            id,
            command_recv,
            packet_recv,
            controller_send,
            fragmenter: Fragmenter::new(),
            assemblers_manager: AssemblersManager::new(),
            topology: Topology::new(id),
            packet_send: HashMap::new(),
            specialized: match server_type {
                ServerType::Chat => Box::new(ChatBehavior::new()),
                ServerType::ContentText => Box::new(TextBehavior::new()),
                ServerType::ContentMedia => Box::new(MediaBehavior::new()),
            },
            fragment_manager: FragmentManager::new(),
            should_terminate: false,
            flood_id: 0,
        }
    }

    /// Runs the main server loop.
    pub fn run(&mut self) {
        while !self.should_terminate {
            select_biased!(
                recv(self.command_recv) -> command => {
                    if let Ok(command) = command {
                        self.handle_command(command);
                    }
                }
                recv(self.packet_recv) -> packet => {
                    if let Ok(packet) = packet {
                        self.handle_packet(packet);
                    }
                },
                default => {
                    if let Some(to_be_sent_fragment) = self.fragment_manager.get_next() {
                        self.send_fragment(to_be_sent_fragment);
                    }
                }
            );
        }
    }

    /// Handles a received command from the controller.
    fn handle_command(&mut self, command: ServerCommand) {
        match command {
            ServerCommand::Crash => self.should_terminate = true,
            ServerCommand::AddDrone(drone_id, sender) => self.add_drone(drone_id, sender),
            ServerCommand::RemoveDrone(drone_id) => self.remove_drone(drone_id),
            ServerCommand::SetMediaPath(path) => self.set_path(path),
        }
    }

    /// Adds a drone to the server's topology and packet senders.
    fn add_drone(&mut self, drone_id: NodeId, sender: Sender<Packet>) {
        self.topology
            .insert_edge((self.id, NodeType::Server), (drone_id, NodeType::Drone));

        match self.packet_send.insert(drone_id, sender) {
            Some(_) => info!("Neighbor {} updated", drone_id),
            None => info!("Neighbor {} added", drone_id),
        }
    }

    /// Removes a drone from the server's topology.
    fn remove_drone(&mut self, drone_id: NodeId) {
        self.topology.remove_edge(self.id, drone_id);
    }

    /// Set the content server path if its possible, otherwise send an error to the controller
    fn set_path(&mut self, path: PathBuf) {
        match self.specialized.set_path(path) {
            Ok(()) => info!("{} Updated path", self.get_prefix()),
            Err(SetPathError::FileSystem(err)) => {
                error!(
                    "{} Cant update his path due to file system error on specified path",
                    self.get_prefix()
                );
                self.send_server_event(ServerEvent::MediaPathError(err));
            }
            Err(SetPathError::WrongServerType) => {
                error!(
                    "{} Cant update his path due to his type (not a content server)",
                    self.get_prefix()
                );
                self.send_server_event(ServerEvent::UnexpectedCommand);
            }
        }
    }

    /// Handles a packet based on its type.
    fn handle_packet(&mut self, packet: Packet) {
        match packet.pack_type {
            PacketType::Ack(ack) => self.handle_ack(ack, packet.session_id, packet.routing_header),
            PacketType::Nack(nack) => {
                self.handle_nack(nack, packet.session_id, packet.routing_header)
            }
            PacketType::MsgFragment(fragment) => {
                self.handle_fragment(fragment, packet.session_id, packet.routing_header)
            }
            PacketType::FloodRequest(flood_req) => {
                self.handle_flood_request(flood_req, packet.session_id)
            }
            PacketType::FloodResponse(flood_res) => self.handle_flood_response(flood_res),
        }
    }

    /// Handles an acknowledgment packet.
    ///
    /// Removes the corresponding fragment from the fragment manager's cache.
    fn handle_ack(&mut self, ack: Ack, session_id: SessionId, header: SourceRoutingHeader) {
        if let Some(sender) = header.hops.get(0) {
            self.topology.observe_success(*sender);
        };

        self.fragment_manager
            .remove_from_cache((session_id, ack.fragment_index));
    }

    /// Handles a negative acknowledgment packet.
    ///
    /// Based on the NACK type, it either reinserts the fragment into the fragment manager's buffer
    /// or initiates network discovery.
    fn handle_nack(&mut self, nack: Nack, session_id: SessionId, header: SourceRoutingHeader) {
        if let Some(sender) = header.hops.get(0) {
            self.topology.observe_failure(*sender);
        };

        match nack.nack_type {
            NackType::Dropped => {
                info!("Nack with NackType::Dropped received");
                let _ = self
                    .fragment_manager
                    .insert_from_cache((session_id, nack.fragment_index));
            }
            NackType::DestinationIsDrone => {
                //self.topology.reset();
                self.start_network_discovery();
                let _ = self
                    .fragment_manager
                    .insert_from_cache((session_id, nack.fragment_index));
            }
            NackType::ErrorInRouting(_) => {
                //self.topology.reset();
                self.start_network_discovery();
                let _ = self
                    .fragment_manager
                    .insert_from_cache((session_id, nack.fragment_index));
            }
            NackType::UnexpectedRecipient(_) => {
                warn!("Nack with NackType::UnexpectedRecipient received")
            }
        }
    }

    /// Handles a fragment packet.
    ///
    /// Attempts to insert the fragment into the assembler manager. If the message assembly is
    /// complete, it retrieves the assembled data and processes it.
    fn handle_fragment(
        &mut self,
        fragment: Fragment,
        session_id: SessionId,
        header: SourceRoutingHeader,
    ) {
        println!("{} Received fragment", self.get_prefix());

        match self
            .assemblers_manager
            .insert_fragment(fragment, session_id)
        {
            Ok(AssemblerStatus::Complete) => {
                match self.assemblers_manager.retrieve_assembled(session_id) {
                    Ok(assembled) => {
                        match header.hops.first() {
                            Some(id) => self.handle_assembled(assembled, *id),
                            None => error!(
                                "{} Received a packet with empty hops vec",
                                self.get_prefix()
                            ),
                        };
                    }
                    Err(RetrieveError::Incomplete) => warn!(
                        "{} Tryed to assemble an incomplete message",
                        self.get_prefix()
                    ),
                    Err(RetrieveError::UnknownSessionId) => warn!(
                        "{} Tryed to assemble a message with an uknown session id",
                        self.get_prefix()
                    ),
                }
            }
            Err(InsertFragmentError::IndexOutOfBounds) => {
                warn!(
                    "{} Tryed to insert a fragment into buffer with an out of bounds index",
                    self.get_prefix()
                );
            }
            Err(InsertFragmentError::CapacityDoesNotMatch) => {
                warn!(
                    "{} Tryed to insert a fragment into buffer with a non matching capacity",
                    self.get_prefix()
                );
            }
            _ => {}
        }
    }

    /// Handles an assembled message.
    ///
    /// Converts the assembled response into fragments and inserts them into the fragment manager.
    fn handle_assembled(&mut self, assembled: Vec<u8>, initiator_id: NodeId) {
        let response = self.specialized.handle_assembled(assembled, initiator_id);
        let fragments = self.fragmenter.to_fragment_vec(response);
        self.fragment_manager.insert_bulk(fragments);
    }

    /// Handles a flood request packet.
    ///
    /// Starts the flood response process.
    fn handle_flood_request(&mut self, flood_req: FloodRequest, session_id: SessionId) {
        self.begin_flood_response(flood_req, session_id);
    }

    /// Begins the flood response process.
    ///
    /// Constructs a flood response packet and sends it back along the discovered path.
    fn begin_flood_response(&self, req: FloodRequest, session_id: u64) {
        let flood_res = FloodResponse {
            flood_id: req.flood_id,
            path_trace: {
                let mut trace = req.path_trace;
                trace.push((self.id, NodeType::Server));
                trace
            },
        };

        let mut hops: Vec<NodeId> = flood_res
            .path_trace
            .iter()
            .map(|(id, _)| *id)
            .rev()
            .collect();

        if let Some(last) = hops.last() {
            if *last != req.initiator_id {
                hops.push(req.initiator_id);
            }
        }

        let flood_res_packet = Packet {
            routing_header: SourceRoutingHeader { hop_index: 1, hops },
            pack_type: PacketType::FloodResponse(flood_res),
            session_id,
        };

        self.send_packet(flood_res_packet);
    }

    /// Handles a flood response packet.
    ///
    /// Updates the network topology based on the path trace provided in the flood response.
    fn handle_flood_response(&mut self, flood_res: FloodResponse) {
        for (node1, node2) in flood_res
            .path_trace
            .iter()
            .zip(flood_res.path_trace.iter().skip(1))
        {
            self.topology.insert_edge(*node1, *node2);
        }
    }

    /// Starts the network discovery process.
    ///
    /// Sends flood request packets to all neighbors to explore the network topology.
    fn start_network_discovery(&mut self) {
        println!("STARTING NET DISCOVERY");
        self.topology.reset();

        for (_, sender) in self.packet_send.iter() {
            let packet = Packet {
                routing_header: SourceRoutingHeader {
                    hop_index: 0,
                    hops: Vec::new(),
                },
                session_id: 0,
                pack_type: PacketType::FloodRequest(FloodRequest {
                    flood_id: self.flood_id,
                    initiator_id: self.id,
                    path_trace: vec![(self.id, NodeType::Server)],
                }),
            };

            self.flood_id += 1;

            self.send_packet_to_sender(packet, sender);
        }
    }

    /// Sends a fragment to its destination using the shortest path in the topology.
    ///
    /// # Behavior
    ///
    /// - If a path to the destination is found, a packet is created with the appropriate routing header.
    /// - If the topology is still updating or no path is found, the fragment is reinserted into the
    ///   fragment manager's buffer.
    /// - If the topology is not updating but no path is found, the network discovery process is started.
    /// - If the fragment is being sent to the server itself just ignore it and log an error.
    fn send_fragment(&mut self, to_be_sent_fragment: ToBeSentFragment) {
        //let path = self.topology.bfs(self.id, to_be_sent_fragment.dest);
        let path = self.topology.dijkstra(self.id, to_be_sent_fragment.dest);

        match path {
            Ok(path) => {
                let header = SourceRoutingHeader {
                    hop_index: 1,
                    hops: path,
                };

                let packet = Packet {
                    pack_type: PacketType::MsgFragment(to_be_sent_fragment.fragment),
                    routing_header: header,
                    session_id: to_be_sent_fragment.session_id,
                };
                self.send_packet(packet);
            }
            Err(RoutingError::SourceIsDest) => {
                error!("{} Cant send a packet to myself", self.get_prefix())
            }
            Err(RoutingError::NoPathFound) => {
                //if the topology is still updating its ok to not find the path
                //=> reinsert the packet in the buffer
                let _ = self.fragment_manager.insert_from_cache((
                    to_be_sent_fragment.session_id,
                    to_be_sent_fragment.fragment.fragment_index,
                ));
                if !self.topology.is_updating() {
                    //self.topology.reset();
                    self.start_network_discovery();
                }
            }
        }
    }

    /// Retrieves the sender for the next hop in the packet's routing path.
    fn get_sender(&self, packet: &Packet) -> Option<&Sender<Packet>> {
        let next_index = packet.routing_header.hop_index;
        let next_id = packet.routing_header.hops.get(next_index)?;
        self.packet_send.get(next_id)
    }

    /// Sends a packet to the next hop in its routing path.
    fn send_packet(&self, packet: Packet) {
        match self.get_sender(&packet) {
            Some(sender) => {
                self.send_packet_to_sender(packet, sender);
            }
            None => {
                let message = format!("{} Could not reach neighbor", self.get_prefix());
                error!("{}", message);
            }
        }
    }

    /// Sends a packet to a specific sender.
    fn send_packet_to_sender(&self, packet: Packet, sender: &Sender<Packet>) {
        if sender.send(packet.clone()).is_ok() {
            self.send_server_event(ServerEvent::PacketSent(packet));
        } else {
            let message = format!("{} The drone is disconnected", self.get_prefix());
            error!("{}", message);
        }
    }

    /// Sends an event to the server controller.
    fn send_server_event(&self, server_event: ServerEvent) {
        if self.controller_send.send(server_event).is_err() {
            let message = format!("{} The controller is disconnected", self.get_prefix());
            error!("{}", message);
        }
    }

    /// Retrieves the server's logging prefix.
    fn get_prefix(&self) -> String {
        format!("[SERVER {}]", self.id)
    }
}

#[cfg(test)]
mod tests {
    use crate::Server;
    use client::client::Client;
    use core::panic;
    use crossbeam_channel::unbounded;
    use postcard::{from_bytes, to_allocvec};
    use rust_roveri::RustRoveri;
    use rust_roveri_api::ChatResponse;
    use rust_roveri_api::{ChatRequest, ClientGuiMessage, ContentResponse, GuiClientMessage};
    use rust_roveri_api::{ClientCommand, ContentRequest};
    use rust_roveri_api::{ClientEvent, ServerCommand};
    use rust_roveri_api::{Request, ServerType};
    use rust_roveri_api::{Response, ServerEvent};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;
    use wg_2024::controller::DroneCommand;
    use wg_2024::controller::DroneEvent;
    use wg_2024::drone::Drone;
    use wg_2024::network::NodeId;
    use wg_2024::packet::Packet;

    #[test]
    fn add_drone_test() {
        // Create drone 1 channels
        const DRONE_1_ID: NodeId = 71;
        let (_controller_send_tx_1, _controller_send_rx_1) = unbounded::<DroneEvent>();
        let (_controller_recv_tx_1, _controller_recv_rx_1) = unbounded::<DroneCommand>();
        let (packet_recv_tx_1, _packet_recv_rx_1) = unbounded::<Packet>();

        //Create Server
        const SERVER_ID: NodeId = 72;
        let (_packet_recv_tx_server, packet_recv_rx_server) = unbounded::<Packet>();
        let (s00, r01) = unbounded::<ServerCommand>();
        let (s10, _r11) = unbounded::<ServerEvent>();

        let mut server = Server::new(SERVER_ID, r01, packet_recv_rx_server, s10, ServerType::Chat);
        let server_handle = thread::spawn(move || {
            server.run();
        });
        let _ = s00.send(ServerCommand::AddDrone(
            DRONE_1_ID,
            packet_recv_tx_1.clone(),
        ));

        thread::sleep(Duration::from_millis(100));
        let _ = s00.send(ServerCommand::Crash);
        assert!(server_handle.join().is_ok(), "Server panicked");
    }

    #[test]
    fn set_media_path() {
        //Create Server
        const SERVER_ID: NodeId = 72;
        let (_packet_recv_tx_server, packet_recv_rx_server) = unbounded::<Packet>();
        let (s00, r01) = unbounded::<ServerCommand>();
        let (s10, _r11) = unbounded::<ServerEvent>();

        let mut server = Server::new(SERVER_ID, r01, packet_recv_rx_server, s10, ServerType::Chat);
        let server_handle = thread::spawn(move || {
            server.run();
        });
        let _ = s00.send(ServerCommand::SetMediaPath(PathBuf::from("/tmp")));

        thread::sleep(Duration::from_millis(100));
        let _ = s00.send(ServerCommand::Crash);
        assert!(server_handle.join().is_ok(), "Server panicked");
    }

    #[test]
    fn server_test() {
        // Create browser
        let (message_sender_tx, message_sender_rx) = unbounded();
        let (message_receiver_tx, message_receiver_rx) = unbounded();

        // Create client channels
        const CLIENT_ID: NodeId = 70;
        let (packet_recv_tx_client, packet_recv_rx_client) = unbounded::<Packet>();
        let (command_recv_tx_client, command_recv_rx_client) = unbounded::<ClientCommand>();
        let (event_send_tx_client, _event_send_rx_client) = unbounded::<ClientEvent>();

        // Create drone 1 channels
        const DRONE_1_ID: NodeId = 71;
        let (controller_send_tx_1, _controller_send_rx_1) = unbounded::<DroneEvent>();
        let (controller_recv_tx_1, controller_recv_rx_1) = unbounded::<DroneCommand>();
        let (packet_recv_tx_1, packet_recv_rx_1) = unbounded::<Packet>();

        // Create server channels
        const SERVER_ID: NodeId = 72;
        let (packet_recv_tx_server, packet_recv_rx_server) = unbounded::<Packet>();
        let (s00, r01) = unbounded::<ServerCommand>();
        let (s10, _r11) = unbounded::<ServerEvent>();

        // Create client
        let mut client = Client::new(
            CLIENT_ID,
            packet_recv_rx_client,
            command_recv_rx_client,
            event_send_tx_client,
            message_sender_rx,
            message_receiver_tx,
        );

        command_recv_tx_client
            .send(ClientCommand::AddDrone(
                DRONE_1_ID,
                packet_recv_tx_1.clone(),
            ))
            .expect("Cannot add client 1 to drone 1 neighbors");
        let client_handle = thread::spawn(move || {
            client.run();
        });

        // Create drone 1
        let packet_send_1 = HashMap::new();
        let mut drone_1 = RustRoveri::new(
            DRONE_1_ID,
            controller_send_tx_1,
            controller_recv_rx_1,
            packet_recv_rx_1.clone(),
            packet_send_1,
            0.0,
        );
        let _ = thread::spawn(move || drone_1.run());
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                CLIENT_ID,
                packet_recv_tx_client.clone(),
            ))
            .expect("Cant add client to drone 1 neighbors");
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                SERVER_ID,
                packet_recv_tx_server.clone(),
            ))
            .expect("Cant add client to drone 1 neighbors");

        // Create server
        let mut server = Server::new(SERVER_ID, r01, packet_recv_rx_server, s10, ServerType::Chat);
        let _ = thread::spawn(move || {
            server.run();
        });
        let _ = s00.send(ServerCommand::AddDrone(
            DRONE_1_ID,
            packet_recv_tx_1.clone(),
        ));

        // CONTENT
        let request = ChatRequest::Register("ciao".to_string(), "cane".to_string());
        let _ = message_sender_tx.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).unwrap(),
        });

        let request = ChatRequest::Register("come".to_string(), "xx".to_string());
        let _ = message_sender_tx.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).unwrap(),
        });

        let request = ChatRequest::Register("ewe".to_string(), "casdcascd".to_string());
        let _ = message_sender_tx.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).unwrap(),
        });

        let request = ChatRequest::Register("cccccc".to_string(), "cascdac".to_string());
        let _ = message_sender_tx.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).unwrap(),
        });

        let data = message_receiver_rx.recv().unwrap();
        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };
        if let Ok(ChatResponse::ClientList(name, vec)) = from_bytes::<ChatResponse>(&data) {
            println!("{} {:?}", name, vec);
        }

        let data = message_receiver_rx.recv().unwrap();
        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };
        if let Ok(ChatResponse::ClientList(name, vec)) = from_bytes::<ChatResponse>(&data) {
            println!("{} {:?}", name, vec);
        }

        let data = message_receiver_rx.recv().unwrap();
        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };
        if let Ok(ChatResponse::ClientList(name, vec)) = from_bytes::<ChatResponse>(&data) {
            println!("{} {:?}", name, vec);
        }

        let data = message_receiver_rx.recv().unwrap();
        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };
        if let Ok(ChatResponse::ClientList(name, vec)) = from_bytes::<ChatResponse>(&data) {
            println!("{} {:?}", name, vec);
        }

        let _ = command_recv_tx_client.send(ClientCommand::Crash);

        thread::sleep(Duration::from_millis(100));
        assert!(client_handle.join().is_ok());
    }

    #[test]
    fn test_dropped_request() {
        // Create browser
        let (message_sender_tx, message_sender_rx) = unbounded();
        let (message_receiver_tx, message_receiver_rx) = unbounded();

        // Create client channels
        const CLIENT_ID: NodeId = 70;
        let (packet_recv_tx_client, packet_recv_rx_client) = unbounded::<Packet>();
        let (command_recv_tx_client, command_recv_rx_client) = unbounded::<ClientCommand>();
        let (event_send_tx_client, _event_send_rx_client) = unbounded::<ClientEvent>();

        // Create drone 1 channels
        const DRONE_1_ID: NodeId = 71;
        let (controller_send_tx_1, _controller_send_rx_1) = unbounded::<DroneEvent>();
        let (controller_recv_tx_1, controller_recv_rx_1) = unbounded::<DroneCommand>();
        let (packet_recv_tx_1, packet_recv_rx_1) = unbounded::<Packet>();

        // Create server channels
        const SERVER_ID: NodeId = 72;
        let (packet_recv_tx_server, packet_recv_rx_server) = unbounded::<Packet>();
        let (command_recv_tx_server, command_recv_rx_server) = unbounded::<ServerCommand>();
        let (event_send_tx_server, _event_send_rx_server) = unbounded::<ServerEvent>();

        // Create client
        let mut client = Client::new(
            CLIENT_ID,
            packet_recv_rx_client,
            command_recv_rx_client,
            event_send_tx_client,
            message_sender_rx,
            message_receiver_tx,
        );
        command_recv_tx_client
            .send(ClientCommand::AddDrone(
                DRONE_1_ID,
                packet_recv_tx_1.clone(),
            ))
            .expect("Cannot add client 1 to drone 1 neighbors");
        let client_handle = thread::spawn(move || {
            client.run();
        });

        // Create drone 1
        let packet_send_1 = HashMap::new();
        let mut drone_1 = RustRoveri::new(
            DRONE_1_ID,
            controller_send_tx_1,
            controller_recv_rx_1,
            packet_recv_rx_1.clone(),
            packet_send_1,
            0.9,
        );

        let _ = thread::spawn(move || drone_1.run());
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                CLIENT_ID,
                packet_recv_tx_client.clone(),
            ))
            .expect("Cant add client to drone 1 neighbors");
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                SERVER_ID,
                packet_recv_tx_server.clone(),
            ))
            .expect("Cant add client to drone 1 neighbors");

        // Create server
        let mut server = Server::new(
            SERVER_ID,
            command_recv_rx_server,
            packet_recv_rx_server,
            event_send_tx_server,
            ServerType::Chat,
        );
        let _ = thread::spawn(move || {
            server.run();
        });
        let _ = command_recv_tx_server.send(ServerCommand::AddDrone(
            DRONE_1_ID,
            packet_recv_tx_1.clone(),
        ));
        let _ = command_recv_tx_server.send(ServerCommand::SetMediaPath(PathBuf::from(".")));

        // Send register request
        let username = "ciao".repeat(200);
        let password = "comeva".repeat(200);
        let request = Request::Chat(ChatRequest::Register(username.clone(), password));
        let _ = message_sender_tx.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).unwrap(),
        });

        // Receive client list
        let data: ClientGuiMessage = message_receiver_rx
            .recv()
            .expect("Client did not receive a Response");

        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };

        match from_bytes::<Response>(&data) {
            Ok(Response::Chat(response)) => {
                match response {
                    ChatResponse::ClientList(username, usernames) => {
                        assert_eq!(username, username);
                        assert_eq!(usernames, vec![username.clone()]);
                    }
                    _ => panic!("ChatResponse is not ClientList, {:?}", response),
                };
            }
            _ => panic!("data received is not a Response"),
        }

        // Crash client
        thread::sleep(Duration::from_millis(1000));
        let _ = command_recv_tx_client.send(ClientCommand::Crash);

        assert!(client_handle.join().is_ok());
    }

    #[test]
    fn test_poisoned() {
        // Topology:
        //   ----- d1 -----
        //  /              \
        // c                s
        //  \              /
        //   -- d2 -- d3 --

        // Set parameters
        const CLIENT_ID: NodeId = 70;
        const DRONE_1_ID: NodeId = 71;
        const DRONE_2_ID: NodeId = 72;
        const DRONE_3_ID: NodeId = 73;
        const SERVER_ID: NodeId = 74;
        const PDR_1: f32 = 1.0;
        const PDR_2: f32 = 0.9;
        const PDR_3: f32 = 0.9;

        // Create browser
        let (message_sender_tx, message_sender_rx) = unbounded();
        let (message_receiver_tx, message_receiver_rx) = unbounded();

        // Create client channels
        let (packet_recv_tx_client, packet_recv_rx_client) = unbounded::<Packet>();
        let (command_recv_tx_client, command_recv_rx_client) = unbounded::<ClientCommand>();
        let (event_send_tx_client, _) = unbounded::<ClientEvent>();

        // Create drone 1 channels
        let (controller_send_tx_1, _controller_send_rx_1) = unbounded::<DroneEvent>();
        let (controller_recv_tx_1, controller_recv_rx_1) = unbounded::<DroneCommand>();
        let (packet_recv_tx_1, packet_recv_rx_1) = unbounded::<Packet>();

        // Create drone 2 channels
        let (controller_send_tx_2, _controller_send_rx_2) = unbounded::<DroneEvent>();
        let (controller_recv_tx_2, controller_recv_rx_2) = unbounded::<DroneCommand>();
        let (packet_recv_tx_2, packet_recv_rx_2) = unbounded::<Packet>();

        // Create drone 3 channels
        let (controller_send_tx_3, _controller_send_rx_3) = unbounded::<DroneEvent>();
        let (controller_recv_tx_3, controller_recv_rx_3) = unbounded::<DroneCommand>();
        let (packet_recv_tx_3, packet_recv_rx_3) = unbounded::<Packet>();

        // Create server channels
        let (packet_recv_tx_server, packet_recv_rx_server) = unbounded::<Packet>();
        let (command_recv_tx_server, command_recv_rx_server) = unbounded::<ServerCommand>();
        let (event_send_tx_server, _) = unbounded::<ServerEvent>();

        // Create client
        let mut client = Client::new(
            CLIENT_ID,
            packet_recv_rx_client,
            command_recv_rx_client,
            event_send_tx_client,
            message_sender_rx,
            message_receiver_tx,
        );
        command_recv_tx_client
            .send(ClientCommand::AddDrone(
                DRONE_1_ID,
                packet_recv_tx_1.clone(),
            ))
            .expect("Cannot add client 1 to drone 1 neighbors");
        command_recv_tx_client
            .send(ClientCommand::AddDrone(
                DRONE_2_ID,
                packet_recv_tx_2.clone(),
            ))
            .expect("Cannot add client 1 to drone 1 neighbors");
        let client_handle = thread::spawn(move || {
            client.run();
        });

        // Create drone 1
        let packet_send_1 = HashMap::new();
        let mut drone_1 = RustRoveri::new(
            DRONE_1_ID,
            controller_send_tx_1,
            controller_recv_rx_1,
            packet_recv_rx_1.clone(),
            packet_send_1,
            PDR_1,
        );
        let _ = thread::spawn(move || drone_1.run());
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                CLIENT_ID,
                packet_recv_tx_client.clone(),
            ))
            .expect("Cannot add client to drone 1 neighbors");
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                SERVER_ID,
                packet_recv_tx_server.clone(),
            ))
            .expect("Cannot add server to drone 1 neighbors");

        // Create drone 2
        let packet_send_2 = HashMap::new();
        let mut drone_2 = RustRoveri::new(
            DRONE_2_ID,
            controller_send_tx_2,
            controller_recv_rx_2,
            packet_recv_rx_2.clone(),
            packet_send_2,
            PDR_2,
        );
        let _ = thread::spawn(move || drone_2.run());
        controller_recv_tx_2
            .send(DroneCommand::AddSender(
                CLIENT_ID,
                packet_recv_tx_client.clone(),
            ))
            .expect("Cannot add client to drone 2 neighbors");
        controller_recv_tx_2
            .send(DroneCommand::AddSender(
                DRONE_3_ID,
                packet_recv_tx_3.clone(),
            ))
            .expect("Cannot add drone 3 to drone 2 neighbors");

        // Create drone 3
        let packet_send_3 = HashMap::new();
        let mut drone_3 = RustRoveri::new(
            DRONE_3_ID,
            controller_send_tx_3,
            controller_recv_rx_3,
            packet_recv_rx_3.clone(),
            packet_send_3,
            PDR_3,
        );
        let _ = thread::spawn(move || drone_3.run());
        controller_recv_tx_3
            .send(DroneCommand::AddSender(
                DRONE_2_ID,
                packet_recv_tx_2.clone(),
            ))
            .expect("Cannot add drone 2 to drone 3 neighbors");
        controller_recv_tx_3
            .send(DroneCommand::AddSender(
                SERVER_ID,
                packet_recv_tx_server.clone(),
            ))
            .expect("Cannot add server to drone 3 neighbors");

        // Create server
        let mut server = Server::new(
            SERVER_ID,
            command_recv_rx_server,
            packet_recv_rx_server,
            event_send_tx_server,
            ServerType::ContentText,
        );
        let _ = thread::spawn(move || {
            server.run();
        });
        command_recv_tx_server
            .send(ServerCommand::AddDrone(
                DRONE_1_ID,
                packet_recv_tx_1.clone(),
            ))
            .expect("Cannot add drone 1 to server neighbors");
        command_recv_tx_server
            .send(ServerCommand::AddDrone(
                DRONE_3_ID,
                packet_recv_tx_3.clone(),
            ))
            .expect("Cannot add drone 3 to server neighbors");
        let _ =
            command_recv_tx_server.send(ServerCommand::SetMediaPath(PathBuf::from("/tmp/ciao")));

        // Send list request
        let request = ContentRequest::List;

        let _ = message_sender_tx.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).expect("Could not convert ContentRequest::List to bytes"),
        });

        // Receive list response
        let data = message_receiver_rx
            .recv()
            .expect("Client did not receive a ContentResponse");

        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };

        match from_bytes::<ContentResponse>(&data) {
            Ok(ContentResponse::List(_)) => {}
            _ => panic!("ContentResponse is not a List"),
        }

        // Crash client
        thread::sleep(Duration::from_millis(100));
        let _ = command_recv_tx_client.send(ClientCommand::Crash);

        assert!(client_handle.join().is_ok());
    }

    #[test]
    fn test_poisoned_2() {
        // Topology:
        //          ----- d2 -----
        //         /              \
        // c --- d1                d5 --- s
        //         \              /
        //          -- d3 -- d4 --

        // Set parameters
        const CLIENT_ID: NodeId = 70;
        const DRONE_1_ID: NodeId = 71;
        const DRONE_2_ID: NodeId = 72;
        const DRONE_3_ID: NodeId = 73;
        const DRONE_4_ID: NodeId = 74;
        const DRONE_5_ID: NodeId = 75;
        const SERVER_ID: NodeId = 76;
        const PDR_1: f32 = 0.9;
        const PDR_2: f32 = 1.0;
        const PDR_3: f32 = 0.9;
        const PDR_4: f32 = 0.9;
        const PDR_5: f32 = 0.9;

        // Create browser
        let (message_sender_tx, message_sender_rx) = unbounded();
        let (message_receiver_tx, message_receiver_rx) = unbounded();

        // Create client channels
        let (packet_recv_tx_client, packet_recv_rx_client) = unbounded::<Packet>();
        let (command_recv_tx_client, command_recv_rx_client) = unbounded::<ClientCommand>();
        let (event_send_tx_client, _event_send_rx_client) = unbounded::<ClientEvent>();

        // Create drone 1 channels
        let (controller_send_tx_1, _controller_send_rx_1) = unbounded::<DroneEvent>();
        let (controller_recv_tx_1, controller_recv_rx_1) = unbounded::<DroneCommand>();
        let (packet_recv_tx_1, packet_recv_rx_1) = unbounded::<Packet>();

        // Create drone 2 channels
        let (controller_send_tx_2, _controller_send_rx_2) = unbounded::<DroneEvent>();
        let (controller_recv_tx_2, controller_recv_rx_2) = unbounded::<DroneCommand>();
        let (packet_recv_tx_2, packet_recv_rx_2) = unbounded::<Packet>();

        // Create drone 3 channels
        let (controller_send_tx_3, _controller_send_rx_3) = unbounded::<DroneEvent>();
        let (controller_recv_tx_3, controller_recv_rx_3) = unbounded::<DroneCommand>();
        let (packet_recv_tx_3, packet_recv_rx_3) = unbounded::<Packet>();

        // Create drone 4 channels
        let (controller_send_tx_4, _controller_send_rx_4) = unbounded::<DroneEvent>();
        let (controller_recv_tx_4, controller_recv_rx_4) = unbounded::<DroneCommand>();
        let (packet_recv_tx_4, packet_recv_rx_4) = unbounded::<Packet>();

        // Create drone 5 channels
        let (controller_send_tx_5, _controller_send_rx_5) = unbounded::<DroneEvent>();
        let (controller_recv_tx_5, controller_recv_rx_5) = unbounded::<DroneCommand>();
        let (packet_recv_tx_5, packet_recv_rx_5) = unbounded::<Packet>();

        // Create server channels
        let (packet_recv_tx_server, packet_recv_rx_server) = unbounded::<Packet>();
        let (command_recv_tx_server, command_recv_rx_server) = unbounded::<ServerCommand>();
        let (event_send_tx_server, _event_send_rx_server) = unbounded::<ServerEvent>();

        // Create client
        let mut client = Client::new(
            CLIENT_ID,
            packet_recv_rx_client,
            command_recv_rx_client,
            event_send_tx_client,
            message_sender_rx,
            message_receiver_tx,
        );
        let client_handle = thread::spawn(move || {
            client.run();
        });
        command_recv_tx_client
            .send(ClientCommand::AddDrone(
                DRONE_1_ID,
                packet_recv_tx_1.clone(),
            ))
            .expect("Cannot add drone 1 to client neighbors");

        // Create drone 1
        let packet_send_1 = HashMap::new();
        let mut drone_1 = RustRoveri::new(
            DRONE_1_ID,
            controller_send_tx_1,
            controller_recv_rx_1,
            packet_recv_rx_1.clone(),
            packet_send_1,
            PDR_1,
        );
        let handle_1 = thread::spawn(move || drone_1.run());
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                CLIENT_ID,
                packet_recv_tx_client.clone(),
            ))
            .expect("Cannot add client to drone 1 neighbors");
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                DRONE_2_ID,
                packet_recv_tx_2.clone(),
            ))
            .expect("Cannot add drone 2 to drone 1 neighbors");
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                DRONE_3_ID,
                packet_recv_tx_3.clone(),
            ))
            .expect("Cannot add drone 3 to drone 1 neighbors");

        // Create drone 2
        let packet_send_2 = HashMap::new();
        let mut drone_2 = RustRoveri::new(
            DRONE_2_ID,
            controller_send_tx_2,
            controller_recv_rx_2,
            packet_recv_rx_2.clone(),
            packet_send_2,
            PDR_2,
        );
        let handle_2 = thread::spawn(move || drone_2.run());
        controller_recv_tx_2
            .send(DroneCommand::AddSender(
                DRONE_1_ID,
                packet_recv_tx_1.clone(),
            ))
            .expect("Cannot add drone 1 to drone 2 neighbors");
        controller_recv_tx_2
            .send(DroneCommand::AddSender(
                DRONE_5_ID,
                packet_recv_tx_5.clone(),
            ))
            .expect("Cannot add drone 5 to drone 2 neighbors");

        // Create drone 3
        let packet_send_3 = HashMap::new();
        let mut drone_3 = RustRoveri::new(
            DRONE_3_ID,
            controller_send_tx_3,
            controller_recv_rx_3,
            packet_recv_rx_3.clone(),
            packet_send_3,
            PDR_3,
        );
        let handle_3 = thread::spawn(move || drone_3.run());
        controller_recv_tx_3
            .send(DroneCommand::AddSender(
                DRONE_1_ID,
                packet_recv_tx_1.clone(),
            ))
            .expect("Cannot add drone 1 to drone 3 neighbors");
        controller_recv_tx_3
            .send(DroneCommand::AddSender(
                DRONE_4_ID,
                packet_recv_tx_4.clone(),
            ))
            .expect("Cannot add drone 4 to drone 3 neighbors");

        // Create drone 4
        let packet_send_4 = HashMap::new();
        let mut drone_4 = RustRoveri::new(
            DRONE_4_ID,
            controller_send_tx_4,
            controller_recv_rx_4,
            packet_recv_rx_4.clone(),
            packet_send_4,
            PDR_4,
        );
        let handle_4 = thread::spawn(move || drone_4.run());
        controller_recv_tx_4
            .send(DroneCommand::AddSender(
                DRONE_3_ID,
                packet_recv_tx_3.clone(),
            ))
            .expect("Cannot add drone 3 to drone 4 neighbors");
        controller_recv_tx_4
            .send(DroneCommand::AddSender(
                DRONE_5_ID,
                packet_recv_tx_5.clone(),
            ))
            .expect("Cannot add drone 5 to drone 2 neighbors");

        // Create drone 5
        let packet_send_5 = HashMap::new();
        let mut drone_5 = RustRoveri::new(
            DRONE_5_ID,
            controller_send_tx_5,
            controller_recv_rx_5,
            packet_recv_rx_5.clone(),
            packet_send_5,
            PDR_5,
        );
        let handle_5 = thread::spawn(move || drone_5.run());
        controller_recv_tx_5
            .send(DroneCommand::AddSender(
                DRONE_2_ID,
                packet_recv_tx_2.clone(),
            ))
            .expect("Cannot add drone 2 to drone 5 neighbors");
        controller_recv_tx_5
            .send(DroneCommand::AddSender(
                DRONE_4_ID,
                packet_recv_tx_4.clone(),
            ))
            .expect("Cannot add drone 4 to drone 5 neighbors");
        controller_recv_tx_5
            .send(DroneCommand::AddSender(
                SERVER_ID,
                packet_recv_tx_server.clone(),
            ))
            .expect("Cannot add server to drone 5 neighbors");

        // Create server
        let mut server = Server::new(
            SERVER_ID,
            command_recv_rx_server,
            packet_recv_rx_server,
            event_send_tx_server,
            ServerType::ContentText,
        );
        let server_handle = thread::spawn(move || {
            server.run();
        });
        command_recv_tx_server
            .send(ServerCommand::AddDrone(
                DRONE_5_ID,
                packet_recv_tx_5.clone(),
            ))
            .expect("Cannot add drone 5 to server neighbors");
        let _ = command_recv_tx_server.send(ServerCommand::SetMediaPath(PathBuf::from(".")));

        thread::sleep(Duration::from_millis(100));

        // Send list request
        let request = Request::Content(ContentRequest::List);
        let _ = message_sender_tx.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).expect("Could not convert ContentRequest::List to bytes"),
        });

        // Receive list response
        let data = message_receiver_rx
            .recv()
            .expect("Client did not receive a Response");

        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };

        match from_bytes::<Response>(&data) {
            Ok(Response::Content(ContentResponse::List(_))) => {}
            _ => panic!("Response is not a ContentResponse of List"),
        }

        // Crash nodes
        let _ = command_recv_tx_client.send(ClientCommand::Crash);
        let _ = controller_recv_tx_1.send(DroneCommand::Crash);
        let _ = controller_recv_tx_2.send(DroneCommand::Crash);
        let _ = controller_recv_tx_3.send(DroneCommand::Crash);
        let _ = controller_recv_tx_4.send(DroneCommand::Crash);
        let _ = controller_recv_tx_5.send(DroneCommand::Crash);
        let _ = command_recv_tx_server.send(ServerCommand::Crash);

        assert!(client_handle.join().is_ok());
        assert!(handle_1.join().is_ok());
        assert!(handle_2.join().is_ok());
        assert!(handle_3.join().is_ok());
        assert!(handle_4.join().is_ok());
        assert!(handle_5.join().is_ok());
        assert!(server_handle.join().is_ok());
    }

    #[test]
    fn test_chat_request() {
        // Topology:
        // c1 ---
        //       \
        //        d1 --- s1
        //       /
        // c2 ---

        // Set parameters
        const CLIENT_1_ID: NodeId = 71;
        const CLIENT_2_ID: NodeId = 72;
        const DRONE_1_ID: NodeId = 73;
        const SERVER_ID: NodeId = 74;
        const PDR: f32 = 0.0;
        let username_1 = "gggggeeeeean".to_string();
        let password_1 = "password1".to_string();
        let username_2 = "marco".to_string();
        let password_2 = "password2".to_string();
        let message_1 = "ciao come stai".to_string();
        let message_2 = "beneeeee".to_string();

        // Create chat 1 client
        let (message_sender_tx_1, message_sender_rx_1) = unbounded();
        let (message_receiver_tx_1, message_receiver_rx_1) = unbounded();

        // Create chat 2 client
        let (message_sender_tx_2, message_sender_rx_2) = unbounded();
        let (message_receiver_tx_2, message_receiver_rx_2) = unbounded();

        // Create client 1 channels
        let (packet_recv_tx_client_1, packet_recv_rx_client_1) = unbounded::<Packet>();
        let (command_recv_tx_client_1, command_recv_rx_client_1) = unbounded::<ClientCommand>();
        let (event_send_tx_client_1, _event_send_rx_client_1) = unbounded::<ClientEvent>();

        // Create client 2 channels
        let (packet_recv_tx_client_2, packet_recv_rx_client_2) = unbounded::<Packet>();
        let (command_recv_tx_client_2, command_recv_rx_client_2) = unbounded::<ClientCommand>();
        let (event_send_tx_client_2, _event_send_rx_client_2) = unbounded::<ClientEvent>();

        // Create drone 1 channels
        let (controller_send_tx_1, _controller_send_rx_1) = unbounded::<DroneEvent>();
        let (controller_recv_tx_1, controller_recv_rx_1) = unbounded::<DroneCommand>();
        let (packet_recv_tx_1, packet_recv_rx_1) = unbounded::<Packet>();

        // Create server channels
        let (packet_recv_tx_server, packet_recv_rx_server) = unbounded::<Packet>();
        let (command_recv_tx_server, command_recv_rx_server) = unbounded::<ServerCommand>();
        let (event_send_tx_server, _event_send_rx_server) = unbounded::<ServerEvent>();

        // Create client 1
        let mut client_1 = Client::new(
            CLIENT_1_ID,
            packet_recv_rx_client_1,
            command_recv_rx_client_1,
            event_send_tx_client_1,
            message_sender_rx_1,
            message_receiver_tx_1,
        );
        command_recv_tx_client_1
            .send(ClientCommand::AddDrone(
                DRONE_1_ID,
                packet_recv_tx_1.clone(),
            ))
            .expect("Cannot add client 1 to drone 1 neighbors");
        let client_1_handle = thread::spawn(move || {
            client_1.run();
        });

        // Create client 2
        let mut client_2 = Client::new(
            CLIENT_2_ID,
            packet_recv_rx_client_2,
            command_recv_rx_client_2,
            event_send_tx_client_2,
            message_sender_rx_2,
            message_receiver_tx_2,
        );
        command_recv_tx_client_2
            .send(ClientCommand::AddDrone(
                DRONE_1_ID,
                packet_recv_tx_1.clone(),
            ))
            .expect("Cannot add client 1 to drone 1 neighbors");
        let client_2_handle = thread::spawn(move || {
            client_2.run();
        });

        // Create drone 1
        let packet_send_1 = HashMap::new();
        let mut drone_1 = RustRoveri::new(
            DRONE_1_ID,
            controller_send_tx_1,
            controller_recv_rx_1,
            packet_recv_rx_1.clone(),
            packet_send_1,
            PDR,
        );
        let _ = thread::spawn(move || drone_1.run());
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                CLIENT_1_ID,
                packet_recv_tx_client_1.clone(),
            ))
            .expect("Cannot add client 1 to drone 1 neighbors");
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                CLIENT_2_ID,
                packet_recv_tx_client_2.clone(),
            ))
            .expect("Cannot add client 2 to drone 1 neighbors");
        controller_recv_tx_1
            .send(DroneCommand::AddSender(
                SERVER_ID,
                packet_recv_tx_server.clone(),
            ))
            .expect("Cannot add client to drone 1 neighbors");

        // Create server
        let mut server = Server::new(
            SERVER_ID,
            command_recv_rx_server,
            packet_recv_rx_server,
            event_send_tx_server,
            ServerType::Chat,
        );
        let _ = thread::spawn(move || {
            server.run();
        });
        let _ = command_recv_tx_server.send(ServerCommand::AddDrone(
            DRONE_1_ID,
            packet_recv_tx_1.clone(),
        ));
        let _ = command_recv_tx_server.send(ServerCommand::SetMediaPath(PathBuf::from(".")));

        // User 1 registers
        let request = Request::Chat(ChatRequest::Register(username_1.clone(), password_1));
        let _ = message_sender_tx_1.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).expect("Could not convert Request to bytes"),
        });

        // User 1 receives client list
        let data = message_receiver_rx_1
            .recv()
            .expect("Client did not receive a Response");

        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };

        match from_bytes::<Response>(&data) {
            Ok(Response::Chat(ChatResponse::ClientList(username, usernames))) => {
                assert_eq!(username, username_1);
                assert_eq!(usernames, vec![username_1.clone()]);
            }
            _ => panic!("Response is not a ContentResponse of ClientList"),
        }

        // User 2 registers
        let request = Request::Chat(ChatRequest::Register(username_2.clone(), password_2));
        let _ = message_sender_tx_2.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).expect("Could not convert Request to bytes"),
        });

        // User 2 receives client list
        let data = message_receiver_rx_2
            .recv()
            .expect("Client did not receive a Response");

        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };

        match from_bytes::<Response>(&data) {
            Ok(Response::Chat(ChatResponse::ClientList(username, usernames))) => {
                assert_eq!(username, username_2);
                assert_eq!(
                    usernames.len(),
                    2,
                    "Client list contains more users than expected"
                );
                assert!(
                    usernames.contains(&username_1),
                    "Client list does not contain user 1"
                );
                assert!(
                    usernames.contains(&username_2),
                    "Client list does not contain user 2"
                );
            }
            _ => panic!("Response is not a ChatResponse of ClientList"),
        }

        // User 1 sends message
        let request = Request::Chat(ChatRequest::Message(
            username_1.clone(),
            username_2.clone(),
            message_1.clone(),
        ));
        let _ = message_sender_tx_1.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).expect("Could not convert Request to bytes"),
        });

        // User 2 receives message
        let data = message_receiver_rx_2
            .recv()
            .expect("User 2 did not receive a Response");

        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };

        match from_bytes::<Response>(&data) {
            Ok(Response::Chat(ChatResponse::Message(username, message))) => {
                assert_eq!(username, username_1);
                assert_eq!(message, message_1);
            }
            _ => panic!("Response is not a ChatResponse of Message"),
        }

        // User 2 sends message
        let request = Request::Chat(ChatRequest::Message(
            username_2.clone(),
            username_1.clone(),
            message_2.clone(),
        ));
        let _ = message_sender_tx_2.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).expect("Could not convert Request to bytes"),
        });

        // User 1 receives message
        let data = message_receiver_rx_1
            .recv()
            .expect("User 1 did not receive a Response");

        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };

        match from_bytes::<Response>(&data) {
            Ok(Response::Chat(ChatResponse::Message(username, message))) => {
                assert_eq!(username, username_2);
                assert_eq!(message, message_2);
            }
            _ => panic!("Response is not a ChatResponse of Message"),
        }

        // User 1 logs out
        let request = Request::Chat(ChatRequest::Logout(username_1.clone()));
        let _ = message_sender_tx_1.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).expect("Could not convert Request to bytes"),
        });

        // User 1 receives ChatResponse
        let data = message_receiver_rx_1
            .recv()
            .expect("User 1 did not receive a Response");

        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };

        match from_bytes::<Response>(&data) {
            Ok(Response::Chat(ChatResponse::LogoutSuccess(username))) => {
                assert_eq!(username, username_1);
            }
            _ => panic!("Response is not a ChatResponse of LogoutSuccess"),
        }

        // User 2 logs out
        let request = Request::Chat(ChatRequest::Logout(username_2.clone()));
        let _ = message_sender_tx_2.send(GuiClientMessage::Message {
            dst: SERVER_ID,
            data: to_allocvec(&request).expect("Could not convert Request to bytes"),
        });

        // User 2 receives ChatResponse
        let data = message_receiver_rx_2
            .recv()
            .expect("User 2 did not receive a Response");

        let data = match data {
            ClientGuiMessage::Message { data, .. } => data,
            _ => panic!("Clientguimessage is not a Message"),
        };

        match from_bytes::<Response>(&data) {
            Ok(Response::Chat(ChatResponse::LogoutSuccess(username))) => {
                assert_eq!(username, username_2);
            }
            _ => panic!("Response is not a ChatResponse of LogoutSuccess"),
        }

        // Crash clients
        thread::sleep(Duration::from_millis(100));
        let _ = command_recv_tx_client_1.send(ClientCommand::Crash);
        let _ = command_recv_tx_client_2.send(ClientCommand::Crash);

        assert!(client_1_handle.join().is_ok());
        assert!(client_2_handle.join().is_ok());
    }
}
