use crate::assemblers_manager::AssemblersManager;
use crate::chat_behavior::ChatBehavior;
use crate::fragment_buffer::{FragmentBufferStatus, InsertFragmentError};
use crate::fragment_manager::{FragmentManager, ToBeSentFragment};
use crate::media_behavior::MediaBehavior;
use crate::specialized_behavior::SpecializedBehavior;
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
    assemblers_manager: AssemblersManager,
    fragment_manager: FragmentManager,
    topology: Topology,
    specialized: Box<dyn SpecializedBehavior>,
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
            );

            if let Some(to_be_sent_fragment) = self.fragment_manager.get_next() {
                self.send_fragment(to_be_sent_fragment);
            }
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
            Err(msg) => todo!("send error to controller"),
        }
    }

    fn handle_packet(&mut self, packet: Packet) {
        match packet.pack_type {
            PacketType::Ack(ack) => self.handle_ack(ack, packet.session_id),
            PacketType::Nack(nack) => self.handle_nack(nack, packet.session_id),
            PacketType::MsgFragment(fragment) => self.handle_fragment(fragment, packet.session_id),
            PacketType::FloodRequest(flood_req) => todo!(),
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
                self.fragment_manager
                    .insert_from_cache((session_id, nack.fragment_index));
            }
            NackType::ErrorInRouting(_) => {
                self.start_network_discovery();
                self.fragment_manager
                    .insert_from_cache((session_id, nack.fragment_index));
            }
            NackType::UnexpectedRecipient(_) => {
                warn!("Nack with NackType::UnexpectedRecipient received")
            }
        }
    }

    fn handle_fragment(&mut self, fragment: Fragment, session_id: SessionId) {
        match self
            .assemblers_manager
            .insert_fragment(fragment, session_id)
        {
            Ok(FragmentBufferStatus::Complete) => {
                match self.assemblers_manager.retrieve_assembled(session_id) {
                    Ok(assembled) => todo!(),
                    Err(_) => todo!(),
                }
            }
            Ok(FragmentBufferStatus::Incomplete) => {
                todo!()
            }
            Err(InsertFragmentError::IndexOutOfBounds) => {
                todo!()
            }
            Err(InsertFragmentError::CapacityDoesNotMatch) => {
                todo!()
            }
        }
    }

    fn handle_flood_response(&mut self, flood_res: FloodResponse) {
        let starter_node = match flood_res.path_trace.get(0) {
            Some(node) => node.0,
            None => todo!(),
        };

        for (node1, node2) in flood_res
            .path_trace
            .iter()
            .zip(flood_res.path_trace.iter().skip(1))
        {
            self.topology.insert_edge(*node1, *node2);
            todo!("be sure to not update the server type");
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
                if self.topology.is_updating() {
                    self.fragment_manager.insert_from_cache((
                        to_be_sent_fragment.session_id,
                        to_be_sent_fragment.fragment.fragment_index,
                    ));
                } else {
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
            panic!("{}", message);
        }
    }

    fn get_prefix(&self) -> String {
        format!("[SERVER {}]", self.id)
    }
}
