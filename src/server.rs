use crate::assemblers_manager::AssemblersManager;
use crate::chat_behavior::ChatBehavior;
use crate::fragment_buffer::{FragmentBufferStatus, InsertFragmentError};
use crate::media_behavior::MediaBehavior;
use crate::specialized_behavior::SpecializedBehavior;
use crate::text_behavior::TextBehavior;
use crate::topology::Topology;
use crossbeam_channel::{select_biased, Receiver, Sender};
use log::{info, warn};
use rust_roveri_api::{ServerCommand, ServerType, SessionId};
use std::collections::HashMap;
use std::path::PathBuf;
use wg_2024::packet::{Ack, Nack, NackType, NodeType};
use wg_2024::{
    network::NodeId,
    packet::{Fragment, Packet, PacketType},
};

pub struct Server {
    id: NodeId,
    command_recv: Receiver<ServerCommand>,
    packet_recv: Receiver<Packet>,
    assemblers_manager: AssemblersManager,

    topology: Topology,

    packet_send: HashMap<NodeId, Sender<Packet>>,
    specialized: Box<dyn SpecializedBehavior>,

    //fragment_buffer: Ci
    should_terminate: bool,
}

impl Server {
    pub fn new(
        id: NodeId,
        command_recv: Receiver<ServerCommand>,
        packet_recv: Receiver<Packet>,
        server_type: ServerType,
    ) -> Self {
        Self {
            id,
            command_recv,
            packet_recv,
            assemblers_manager: AssemblersManager::new(),
            topology: Topology::new(),

            packet_send: HashMap::new(),

            specialized: match server_type {
                ServerType::Chat => Box::new(ChatBehavior::new()),
                ServerType::ContentText => Box::new(TextBehavior::new()),
                ServerType::ContentMedia => Box::new(MediaBehavior::new()),
            },

            should_terminate: false,
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
            )
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
            Ok(()) => todo!(),
            Err(msg) => todo!("send error to controller"),
        }
    }

    fn handle_packet(&mut self, packet: Packet) {
        match packet.pack_type {
            PacketType::Ack(ack) => self.handle_ack(ack),
            PacketType::Nack(nack) => todo!(),
            PacketType::MsgFragment(fragment) => self.handle_fragment(fragment, packet.session_id),
            PacketType::FloodRequest(flood_req) => todo!(),
            PacketType::FloodResponse(flood_res) => todo!(),
        }
    }

    fn handle_ack(&mut self, ack: Ack) {
        todo!()
    }

    fn handle_nack(&mut self, nack: Nack) {
        match nack.nack_type {
            NackType::Dropped => {
                info!("Dropped nack received")
            }
            NackType::DestinationIsDrone => {
                self.topology.reset();
                todo!("begin network discovery protocol")
            }
            NackType::ErrorInRouting(_) => {
                self.topology.reset();
                todo!("begin network discovery protocol")
            }
            NackType::UnexpectedRecipient(_) => {
                warn!("UnexpectedRecipient nack received")
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
}
