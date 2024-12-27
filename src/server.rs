use crossbeam_channel::{select_biased, Receiver};
use rust_roveri_api::ServerCommand;
use wg_2024::{network::NodeId, packet::Packet};

pub struct Server {
    id: NodeId,
    command_recv: Receiver<ServerCommand>,
    packet_recv: Receiver<Packet>,
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

    fn handle_packet(&self, packet: Packet) {
        todo!()
    }
}
