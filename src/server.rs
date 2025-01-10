use crate::assembler::{AssemblerStatus, InsertFragmentError, RetrieveError};
use crate::assemblers_manager::AssemblersManager;
use crate::chat_behavior::ChatBehavior;
use crate::fragment_manager::{FragmentManager, ToBeSentFragment};
use crate::fragmenter::Fragmenter;
use crate::media_behavior::MediaBehavior;
use crate::specialized_behavior::{SetPathError, SpecializedBehavior};
use crate::text_behavior::TextBehavior;
use crate::topology::{RoutingError, Topology};
use core::panic;
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
            topology: Topology::new(),
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

    fn handle_command(&mut self, command: ServerCommand) {
        match command {
            ServerCommand::Crash => self.should_terminate = true,
            ServerCommand::AddDrone(drone_id, sender) => self.add_drone(drone_id, sender),
            ServerCommand::RemoveDrone(drone_id) => self.remove_drone(drone_id),
            ServerCommand::SetMediaPath(path) => self.set_path(path),
        }
    }

    fn add_drone(&mut self, drone_id: NodeId, sender: Sender<Packet>) {
        self.topology
            .insert_edge((self.id, NodeType::Server), (drone_id, NodeType::Drone));

        match self.packet_send.insert(drone_id, sender) {
            Some(_) => info!("Neighbor {} updated", drone_id),
            None => info!("Neighbor {} added", drone_id),
        }
    }

    fn remove_drone(&mut self, drone_id: NodeId) {
        self.topology.remove_edge(self.id, drone_id);
    }

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

    fn handle_packet(&mut self, packet: Packet) {
        match packet.pack_type {
            PacketType::Ack(ack) => self.handle_ack(ack, packet.session_id),
            PacketType::Nack(nack) => self.handle_nack(nack, packet.session_id),
            PacketType::MsgFragment(fragment) => {
                self.handle_fragment(fragment, packet.session_id, packet.routing_header)
            }
            PacketType::FloodRequest(flood_req) => {
                self.handle_flood_request(flood_req, packet.session_id)
            }
            PacketType::FloodResponse(flood_res) => self.handle_flood_response(flood_res),
        }
    }

    fn handle_ack(&mut self, ack: Ack, session_id: SessionId) {
        self.fragment_manager
            .remove_from_cache((session_id, ack.fragment_index));
    }

    fn handle_nack(&mut self, nack: Nack, session_id: SessionId) {
        match nack.nack_type {
            NackType::Dropped => {
                info!("Nack with NackType::Dropped received")
            }
            NackType::DestinationIsDrone => {
                self.start_network_discovery();
                let _ = self
                    .fragment_manager
                    .insert_from_cache((session_id, nack.fragment_index));
            }
            NackType::ErrorInRouting(_) => {
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

    fn handle_fragment(
        &mut self,
        fragment: Fragment,
        session_id: SessionId,
        header: SourceRoutingHeader,
    ) {
        match self
            .assemblers_manager
            .insert_fragment(fragment, session_id)
        {
            Ok(AssemblerStatus::Complete) => {
                match self.assemblers_manager.retrieve_assembled(session_id) {
                    Ok(assembled) => {
                        let initiator_id = match header.hops.first() {
                            Some(hop) => hop,
                            None => panic!(
                                "{} Received a packet with empty hops vec",
                                self.get_prefix()
                            ),
                        };
                        self.handle_assembled(assembled, *initiator_id);
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

    fn handle_assembled(&mut self, assembled: Vec<u8>, initiator_id: NodeId) {
        let response = self.specialized.handle_assembled(assembled, initiator_id);
        let fragments = self.fragmenter.to_fragment_vec(response);
        self.fragment_manager.insert_bulk(fragments);
    }

    fn handle_flood_request(&mut self, flood_req: FloodRequest, session_id: SessionId) {
        self.begin_flood_response(flood_req, session_id);
    }

    fn begin_flood_response(&self, req: FloodRequest, session_id: u64) {
        let flood_res = FloodResponse {
            flood_id: req.flood_id,
            path_trace: {
                let mut trace = req.path_trace;
                trace.push((self.id, NodeType::Server));
                trace
            },
        };

        let flood_res_packet = Packet {
            routing_header: SourceRoutingHeader {
                hop_index: 1,
                hops: flood_res
                    .path_trace
                    .iter()
                    .map(|(id, _)| *id)
                    .rev()
                    .collect(),
            },
            pack_type: PacketType::FloodResponse(flood_res),
            session_id,
        };

        self.send_packet(flood_res_packet);
    }

    fn handle_flood_response(&mut self, flood_res: FloodResponse) {
        for (node1, node2) in flood_res
            .path_trace
            .iter()
            .zip(flood_res.path_trace.iter().skip(1))
        {
            self.topology.insert_edge(*node1, *node2);
        }
    }

    fn start_network_discovery(&mut self) {
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

    fn send_fragment(&mut self, to_be_sent_fragment: ToBeSentFragment) {
        let path = self.topology.bfs(self.id, to_be_sent_fragment.dest);

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
                panic!("Cant send a packet to myself")
            }
            Err(RoutingError::NoPathFound) => {
                //if the topology is still updating its ok to not find the path
                //=> reinsert the packet in the buffer
                let _ = self.fragment_manager.insert_from_cache((
                    to_be_sent_fragment.session_id,
                    to_be_sent_fragment.fragment.fragment_index,
                ));
                if !self.topology.is_updating() {
                    self.start_network_discovery();
                }
            }
        }
    }

    fn get_sender(&self, packet: &Packet) -> Option<&Sender<Packet>> {
        let next_index = packet.routing_header.hop_index;
        let next_id = packet.routing_header.hops.get(next_index)?;
        self.packet_send.get(next_id)
    }

    fn send_packet(&self, packet: Packet) {
        match self.get_sender(&packet) {
            Some(sender) => {
                self.send_packet_to_sender(packet, sender);
            }
            None => {
                let message = format!("{} Could not reach neighbor", self.get_prefix());
                error!("{}", message);
                panic!("{}", message);
            }
        }
    }

    fn send_packet_to_sender(&self, packet: Packet, sender: &Sender<Packet>) {
        if sender.send(packet.clone()).is_ok() {
            self.send_server_event(ServerEvent::PacketSent(packet));
        } else {
            let message = format!("{} The drone is disconnected", self.get_prefix());
            error!("{}", message);
            panic!("{}", message);
        }
    }

    fn send_server_event(&self, server_event: ServerEvent) {
        if self.controller_send.send(server_event).is_err() {
            let message = format!("{} The controller is disconnected", self.get_prefix());
            error!("{}", message);
            //panic!("{}", message);
        }
    }

    fn get_prefix(&self) -> String {
        format!("[SERVER {}]", self.id)
    }
}

#[cfg(test)]
mod tests {
    use crate::Server;
    use client::client::Client;
    use crossbeam_channel::unbounded;
    use crossbeam_channel::{select_biased, Receiver, Sender};
    use postcard::{from_bytes, to_allocvec};
    use rust_roveri::RustRoveri;
    use rust_roveri_api::ContentResponse;
    use rust_roveri_api::ContentType;
    use rust_roveri_api::ServerCommand;
    use rust_roveri_api::ServerEvent;
    use rust_roveri_api::ServerType;
    use rust_roveri_api::{ClientCommand, ContentRequest};
    use rust_roveri_api::{ContentId, FloodId, FragmentId, GuiMessage, SessionId};
    use std::collections::hash_map::Entry;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;
    use wg_2024::controller::DroneCommand;
    use wg_2024::controller::DroneEvent;
    use wg_2024::drone::Drone;
    use wg_2024::network::{NodeId, SourceRoutingHeader};
    use wg_2024::packet::{
        Ack, FloodRequest, FloodResponse, Fragment, Nack, NackType, NodeType, Packet, PacketType,
        FRAGMENT_DSIZE,
    };

    #[test]
    fn add_drone_test() {
        // Create drone 1 channels
        const DRONE_1_ID: NodeId = 71;
        let (controller_send_tx_1, _controller_send_rx_1) = unbounded::<DroneEvent>();
        let (controller_recv_tx_1, controller_recv_rx_1) = unbounded::<DroneCommand>();
        let (packet_recv_tx_1, packet_recv_rx_1) = unbounded::<Packet>();

        //Create Server
        const SERVER_ID: NodeId = 72;
        let (packet_recv_tx_server, packet_recv_rx_server) = unbounded::<Packet>();
        let (s00, r01) = unbounded::<ServerCommand>();
        let (s10, r11) = unbounded::<ServerEvent>();

        let mut server = Server::new(SERVER_ID, r01, packet_recv_rx_server, s10, ServerType::Chat);
        let server_handle = thread::spawn(move || {
            server.run();
        });
        s00.send(ServerCommand::AddDrone(
            DRONE_1_ID,
            packet_recv_tx_1.clone(),
        ));

        thread::sleep(Duration::from_millis(100));
        s00.send(ServerCommand::Crash);
        assert!(server_handle.join().is_ok(), "Server panicked");
    }

    #[test]
    fn set_media_path() {
        //Create Server
        const SERVER_ID: NodeId = 72;
        let (packet_recv_tx_server, packet_recv_rx_server) = unbounded::<Packet>();
        let (s00, r01) = unbounded::<ServerCommand>();
        let (s10, r11) = unbounded::<ServerEvent>();

        let mut server = Server::new(SERVER_ID, r01, packet_recv_rx_server, s10, ServerType::Chat);
        let server_handle = thread::spawn(move || {
            server.run();
        });
        s00.send(ServerCommand::SetMediaPath(PathBuf::from("/tmp")));

        thread::sleep(Duration::from_millis(100));
        s00.send(ServerCommand::Crash);
        assert!(server_handle.join().is_ok(), "Server panicked");
    }

    #[test]
    fn server_test() {
        // Create browser
        let (message_sender_tx, message_sender_rx) = unbounded();
        let (message_receiver_tx, message_receiver_rx) = unbounded();
        //let browser = Browser::new(message_sender_tx.clone(), message_receiver_rx);

        // Create client channels
        const CLIENT_ID: NodeId = 70;
        let (packet_recv_tx_client, packet_recv_rx_client) = unbounded::<Packet>();
        let (command_recv_tx_client, command_recv_rx_client) = unbounded::<ClientCommand>();

        // Create drone 1 channels
        const DRONE_1_ID: NodeId = 71;
        let (controller_send_tx_1, _controller_send_rx_1) = unbounded::<DroneEvent>();
        let (controller_recv_tx_1, controller_recv_rx_1) = unbounded::<DroneCommand>();
        let (packet_recv_tx_1, packet_recv_rx_1) = unbounded::<Packet>();

        // Create server channels
        const SERVER_ID: NodeId = 72;
        let (packet_recv_tx_server, packet_recv_rx_server) = unbounded::<Packet>();
        let (s00, r01) = unbounded::<ServerCommand>();
        let (s10, r11) = unbounded::<ServerEvent>();

        // Create client
        let mut client = Client::new(
            CLIENT_ID,
            packet_recv_rx_client,
            command_recv_rx_client,
            message_sender_rx,
            message_receiver_tx,
        );
        client.add_drone(DRONE_1_ID, packet_recv_tx_1.clone());
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
        let handle_1 = thread::spawn(move || drone_1.run());
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
            r01,
            packet_recv_rx_server,
            s10,
            ServerType::ContentText,
        );
        let server_handle = thread::spawn(move || {
            server.run();
        });
        s00.send(ServerCommand::AddDrone(
            DRONE_1_ID,
            packet_recv_tx_1.clone(),
        ));
        s00.send(ServerCommand::SetMediaPath(PathBuf::from("/tmp/")));

        // LIST
        //let request = ContentRequest::List;
        //message_sender_tx.send((SERVER_ID, to_allocvec(&request).unwrap()));
        //let (id, data) = message_receiver_rx.recv().unwrap();
        //if let Ok(ContentResponse::List(names)) = from_bytes::<ContentResponse>(&data) {
        //    println!("{:?}", names);
        //}

        // CONTENT
        let request = ContentRequest::Content("ciao.txt".to_string());
        message_sender_tx.send((SERVER_ID, to_allocvec(&request).unwrap()));
        let (id, data) = message_receiver_rx.recv().unwrap();
        if let Ok(ContentResponse::Content(name, ctype, bytes)) =
            from_bytes::<ContentResponse>(&data)
        {
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            println!("{}", text);
        }

        //command_recv_tx_client.send(ClientCommand::Crash);

        thread::sleep(Duration::from_millis(100));
        assert!(client_handle.join().is_ok());
    }
}
