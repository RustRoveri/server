//! Defines traits and structures for processing and handling specialized server behaviors.

use log::error;
use postcard::{self, from_bytes, to_allocvec};
use rust_roveri_api::{ContentResponse, Request, Response};
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
    fn handle_assembled(&mut self, assembled: Vec<u8>, initiator_id: NodeId) -> AssembledResponse {
        let request = match from_bytes::<Request>(&assembled).map_err(ProcessError::Deserialize) {
            Ok(request) => request,
            Err(err) => return self.handle_error(err, initiator_id),
        };

        let (response, dest) = match self.process_assembled(request, initiator_id) {
            Ok((response, dest)) => (response, dest),
            Err(err) => return self.handle_error(err, initiator_id),
        };

        let assembled_response = match to_allocvec(&response).map_err(ProcessError::Serialize) {
            Ok(assembled_response) => assembled_response,
            Err(err) => return self.handle_error(err, initiator_id),
        };

        AssembledResponse {
            data: assembled_response,
            dest,
        }
    }

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
        assembled: Request,
        initiator_id: NodeId,
    ) -> Result<(Response, NodeId), ProcessError>;

    fn handle_error(&self, err: ProcessError, dest_id: NodeId) -> AssembledResponse {
        let error_message = match err {
            ProcessError::UnexpectedRequest => format!("Unexpected request"),
            ProcessError::Deserialize(_) => format!("Deserialization error"),
            ProcessError::Serialize(_) => format!("Serialization error"),
            ProcessError::FileSystem(_) => format!("Filesystem error"),
        };
        let error_response = Response::Content(ContentResponse::InternalServerError(error_message));

        match to_allocvec(&error_response) {
            Ok(bytes) => AssembledResponse {
                data: bytes,
                dest: dest_id,
            },
            Err(e) => {
                error!("Failed to serialize error response: {:?}", e);
                AssembledResponse {
                    data: Vec::new(),
                    dest: dest_id,
                }
            }
        }
    }
}
