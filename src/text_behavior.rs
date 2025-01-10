use crate::specialized_behavior::{
    AssembledResponse, ProcessError, SetPathError, SpecializedBehavior,
};
use log::error;
use postcard::{from_bytes, to_allocvec};
use rust_roveri_api::{ContentName, ContentRequest, ContentResponse, ContentType};
use std::{fs, path::PathBuf};
use wg_2024::network::NodeId;

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
                    ProcessError::Deserialize(_) => format!("Deserialization error"),
                    ProcessError::Serialize(_) => format!("Serialization error"),
                    ProcessError::FileSystem(_) => format!("Filesystem error"),
                };
                let error_response = ContentResponse::InternalServerError(error_message);

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

    fn process_assembled(
        &mut self,
        assembled: Vec<u8>,
        initiator_id: NodeId,
    ) -> Result<AssembledResponse, ProcessError> {
        let content_request =
            from_bytes::<ContentRequest>(&assembled).map_err(ProcessError::Deserialize)?;

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

                Ok(AssembledResponse {
                    data: to_allocvec(&response).map_err(ProcessError::Serialize)?,
                    dest: initiator_id,
                })
            }
        }
    }
}
