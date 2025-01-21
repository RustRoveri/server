//! Defines traits and structures for processing and handling specialized server behaviors.

use postcard;
use rust_roveri_api::Request;
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
    UnexpectedRequest,
    Deserialize(postcard::Error),
    Serialize(postcard::Error),
    FileSystem(io::Error),
}

pub trait SpecializedBehavior: Send {
    fn set_path(&mut self, _: PathBuf) -> Result<(), SetPathError> {
        Err(SetPathError::WrongServerType)
    }

    /// Handles incoming assembled data and use `process_assembled` to process requests.
    ///
    /// # Arguments
    ///
    /// * `assembled` - The assembled message (request).
    /// * `initiator_id` - The id of the message sender.
    ///
    /// # Returns
    ///
    /// - `AssembledResponse` the assembled message (response).
    fn handle_assembled(&mut self, assembled: Vec<u8>, initiator_id: NodeId) -> AssembledResponse;

    /// Processes requests based on the behavior and generates appropriate responses.
    /// Errors are propagated to the `handle_assembled` method.
    ///
    /// # Arguments
    ///
    /// * `assembled` - The assembled message (request).
    /// * `initiator_id` - The id of the message sender.
    ///
    /// # Returns
    ///
    /// - `AssembledResponse` the assembled message (response).
    /// - `ProcessError` the error, if it occurs.
    fn process_assembled(
        &mut self,
        assembled: Vec<u8>,
        initiator_id: NodeId,
    ) -> Result<AssembledResponse, ProcessError>;
}
