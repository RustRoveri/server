//! Implements the `TextBehavior` struct for managing text content requests.

use crate::specialized_behavior::{
    AssembledResponse, ProcessError, SetPathError, SpecializedBehavior,
};
use log::error;
use postcard::{from_bytes, to_allocvec};
use rust_roveri_api::{
    ContentName, ContentRequest, ContentResponse, ContentType, Request, Response,
};
use std::{fs, path::PathBuf};
use wg_2024::network::NodeId;

/// Manages text-related requests, such as listing or retrieving text files.
///
/// The `TextBehavior` struct implements the `SpecializedBehavior` trait to handle
/// requests for text content stored in a specified directory.
pub struct TextBehavior {
    path: PathBuf,
}

impl TextBehavior {
    pub fn new() -> Self {
        Self {
            path: PathBuf::new(),
        }
    }
}

impl SpecializedBehavior for TextBehavior {
    /// Sets the directory path for text content.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the directory containing text files.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the directory exists and is accessible.
    /// - `Err(SetPathError::FileSystem)` if there is an error accessing the directory.
    fn set_path(&mut self, path: PathBuf) -> Result<(), SetPathError> {
        let _ = fs::read_dir(&path).map_err(SetPathError::FileSystem)?;

        self.path = path;
        Ok(())
    }

    fn handle_assembled(&mut self, assembled: Vec<u8>, initiator_id: NodeId) -> AssembledResponse {
        match self.process_assembled(assembled, initiator_id) {
            Ok(response) => return response,
            Err(err) => {
                let error_message = match err {
                    ProcessError::UnexpectedRequest => format!("Unexpected request"),
                    ProcessError::Deserialize(_) => format!("Deserialization error"),
                    ProcessError::Serialize(_) => format!("Serialization error"),
                    ProcessError::FileSystem(_) => format!("Filesystem error"),
                };
                let error_response =
                    Response::Content(ContentResponse::InternalServerError(error_message));

                match to_allocvec(&error_response) {
                    Ok(bytes) => AssembledResponse {
                        data: bytes,
                        dest: initiator_id,
                    },
                    Err(e) => {
                        error!("Failed to serialize error response: {:?}", e);
                        AssembledResponse {
                            data: Vec::new(),
                            dest: initiator_id,
                        }
                    }
                }
            }
        }
    }

    /// Processes a request and generates an appropriate response.
    ///
    /// # Arguments
    ///
    /// * `assembled` - The serialized request data.
    /// * `initiator_id` - The ID of the node that sent the request.
    ///
    /// # Returns
    ///
    /// - `Ok(AssembledResponse)` if the request is successfully processed.
    /// - `Err(ProcessError)` if an error occurs during request processing.
    ///
    /// # Behavior
    ///
    /// This method supports two types of requests:
    ///
    /// 1. **List**:
    ///    - Lists all text files in the configured directory.
    ///    - Returns a `ContentResponse::List` containing the file names.
    /// 2. **Content**:
    ///    - Retrieves a specific text file by name.
    ///    - Returns a `ContentResponse::Content` containing the file's data.
    ///
    /// If the requested file is not found or is not a valid file, a `ContentResponse::ContentNotFound`
    /// is returned.
    fn process_assembled(
        &mut self,
        assembled: Vec<u8>,
        initiator_id: NodeId,
    ) -> Result<AssembledResponse, ProcessError> {
        let request = from_bytes::<Request>(&assembled).map_err(ProcessError::Deserialize)?;

        let content_request = match request {
            Request::Content(req) => req,
            _ => return Err(ProcessError::UnexpectedRequest),
        };

        match content_request {
            ContentRequest::List => {
                let entries = fs::read_dir(&self.path).map_err(ProcessError::FileSystem)?;

                let content_names = entries
                    .filter_map(|entry_res| match entry_res {
                        Ok(entry) => {
                            let file_name = entry.file_name();
                            match file_name.into_string() {
                                Ok(name) => Some(name),
                                Err(_) => None,
                            }
                        }
                        Err(_) => None,
                    })
                    .collect::<Vec<ContentName>>();

                let response = ContentResponse::List(content_names);
                let response = Response::Content(response);

                Ok(AssembledResponse {
                    data: to_allocvec(&response).map_err(|err| ProcessError::Serialize(err))?,
                    dest: initiator_id,
                })
            }
            ContentRequest::Content(content_name) => {
                let mut content_path = self.path.clone();
                content_path.push(&content_name);

                if !content_path.exists() || !content_path.is_file() {
                    let response = ContentResponse::ContentNotFound(content_name);
                    return Ok(AssembledResponse {
                        data: to_allocvec(&response).map_err(ProcessError::Serialize)?,
                        dest: initiator_id,
                    });
                }

                let content_data = fs::read(&content_path).map_err(ProcessError::FileSystem)?;
                let response =
                    ContentResponse::Content(content_name, ContentType::Text, content_data);
                let response = Response::Content(response);

                Ok(AssembledResponse {
                    data: to_allocvec(&response).map_err(ProcessError::Serialize)?,
                    dest: initiator_id,
                })
            }
        }
    }
}
