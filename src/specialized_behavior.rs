use postcard;
use std::io;
use std::path::PathBuf;
use wg_2024::network::NodeId;

#[derive(Debug)]
pub struct AssembledResponse {
    pub data: Vec<u8>,
    pub dest: NodeId,
}

pub enum SetPathError {
    WrongServerType,
    FileSystem(io::Error),
}

pub enum ProcessError {
    Deserialize(postcard::Error),
    Serialize(postcard::Error),
    FileSystem(io::Error),
}

pub trait SpecializedBehavior: Send {
    fn set_path(&mut self, _: PathBuf) -> Result<(), SetPathError> {
        Err(SetPathError::WrongServerType)
    }

    fn handle_assembled(&mut self, assembled: Vec<u8>, initiator_id: NodeId) -> AssembledResponse;
    fn process_assembled(
        &mut self,
        assembled: Vec<u8>,
        initiator_id: NodeId,
    ) -> Result<AssembledResponse, ProcessError>;
}
