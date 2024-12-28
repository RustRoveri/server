use crate::fragment_buffer::{FragmentBufferStatus, InsertFragmentError};
use crate::session_buffer_manager::SessionBufferManager;
use crossbeam_channel::{select_biased, Receiver};
use rust_roveri_api::{ServerCommand, SessionId};
use wg_2024::{
    network::NodeId,
    packet::{Fragment, Packet, PacketType},
};

pub struct Server {
    id: NodeId,
    command_recv: Receiver<ServerCommand>,
    packet_recv: Receiver<Packet>,
    session_buffer_manager: SessionBufferManager,
}

impl Server {
    pub fn new(
        id: NodeId,
        command_recv: Receiver<ServerCommand>,
        packet_recv: Receiver<Packet>,
    ) -> Self {
        Self {
            id,
            command_recv,
            packet_recv,
            session_buffer_manager: SessionBufferManager::new(),
        }
    }

    pub fn run(&mut self) {
        loop {
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

    fn handle_command(&self, command: ServerCommand) {
        todo!()
    }

    fn handle_packet(&mut self, packet: Packet) {
        match packet.pack_type {
            PacketType::Ack(ack) => todo!(),
            PacketType::Nack(nack) => todo!(),
            PacketType::MsgFragment(fragment) => self.handle_fragment(fragment, packet.session_id),
            PacketType::FloodRequest(flood_req) => todo!(),
            PacketType::FloodResponse(flood_res) => todo!(),
        }
    }

    fn handle_fragment(&mut self, fragment: Fragment, session_id: SessionId) {
        let res = self
            .session_buffer_manager
            .insert_fragment(fragment, session_id);

        match res {
            Ok(status) => match status {
                FragmentBufferStatus::Complete => todo!(),
                FragmentBufferStatus::Incomplete => todo!(),
            },
            Err(err) => match err {
                InsertFragmentError::IndexOutOfBounds => todo!(),
                InsertFragmentError::CapacityDoesNotMatch => todo!(),
            },
        }
    }
}
