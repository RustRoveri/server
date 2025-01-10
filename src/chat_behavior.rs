use crate::specialized_behavior::{AssembledResponse, ProcessError, SpecializedBehavior};
use log::error;
use postcard::{from_bytes, to_allocvec};
use rust_roveri_api::{
    ChatRequest, ChatResponse, LoginError, LogoutError, MessageError, Password, RegisterError,
    UserName,
};
use std::collections::{hash_map::Entry, HashMap};
use wg_2024::network::NodeId;

type Logged = bool;

pub struct ChatBehavior {
    clients: HashMap<UserName, (Password, NodeId, Logged)>,
}

impl ChatBehavior {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    fn get_client_list(&self) -> Vec<UserName> {
        self.clients
            .iter()
            .map(|(username, _)| username.clone())
            .collect()
    }

    fn register(
        &mut self,
        username: UserName,
        password: Password,
        node_id: NodeId,
    ) -> Result<(), RegisterError> {
        match self.clients.entry(username) {
            Entry::Vacant(entry) => {
                entry.insert((password, node_id, true));
                Ok(())
            }
            Entry::Occupied(_) => Err(RegisterError::AlreadyRegistered),
        }
    }

    fn is_auth(&self, username: &UserName, node_id: NodeId) -> bool {
        match self.clients.get(username) {
            Some((_, id, logged)) => node_id == *id && *logged,
            None => false,
        }
    }

    fn is_logged(&self, username: &UserName) -> bool {
        match self.clients.get(username) {
            Some((_, _, logged)) => *logged,
            None => false,
        }
    }

    fn can_send_message(
        &mut self,
        sender: UserName,
        node_id: NodeId,
        recipient: UserName,
    ) -> Result<(NodeId, UserName), MessageError> {
        if !self.is_auth(&sender, node_id) {
            return Err(MessageError::SenderNotLogged(sender));
        }

        match self.clients.entry(recipient.clone()) {
            Entry::Occupied(entry) => {
                let client = entry.get();

                if client.2 {
                    Ok((client.1, sender))
                } else {
                    Err(MessageError::RecipientNotLogged(recipient))
                }
            }
            Entry::Vacant(_) => Err(MessageError::RecipientNotRegistered(recipient)),
        }
    }

    pub fn login(&mut self, username: &UserName, password: &Password) -> Result<(), LoginError> {
        match self.clients.entry(username.clone()) {
            Entry::Occupied(mut entry) => {
                let client = entry.get_mut();
                if client.2 {
                    Err(LoginError::AlreadyLogged)
                } else if &client.0 != password {
                    Err(LoginError::WrongPassword)
                } else {
                    client.2 = true;
                    Ok(())
                }
            }
            Entry::Vacant(_) => Err(LoginError::NotRegistered),
        }
    }

    pub fn logout(&mut self, username: &UserName, node_id: NodeId) -> Result<(), LogoutError> {
        match self.clients.entry(username.clone()) {
            Entry::Occupied(mut entry) => {
                let client = entry.get_mut();
                if !client.2 {
                    return Err(LogoutError::NotLogged);
                }
                if client.1 != node_id {
                    return Err(LogoutError::InvalidNodeId);
                }
                client.2 = false;
                Ok(())
            }
            Entry::Vacant(_) => Err(LogoutError::NotRegistered),
        }
    }
}

impl SpecializedBehavior for ChatBehavior {
    fn handle_assembled(&mut self, assembled: Vec<u8>, initiator_id: NodeId) -> AssembledResponse {
        match self.process_assembled(assembled, initiator_id) {
            Ok(response) => return response,
            Err(err) => {
                let error_message = match err {
                    ProcessError::Deserialize(_) => format!("Deserialization error"),
                    ProcessError::Serialize(_) => format!("Serialization error"),
                    ProcessError::FileSystem(_) => format!("Filesystem error"),
                };
                let error_response = ChatResponse::InternalServerError(error_message);

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
            from_bytes::<ChatRequest>(&assembled).map_err(ProcessError::Deserialize)?;

        match content_request {
            ChatRequest::ClientList(username) => {
                let response = ChatResponse::ClientList(username, self.get_client_list());
                return Ok(AssembledResponse {
                    data: to_allocvec(&response).map_err(ProcessError::Serialize)?,
                    dest: initiator_id,
                });
            }
            ChatRequest::Message(sender, recipient, msg) => {
                let (response, dest) = match self.can_send_message(sender, initiator_id, recipient)
                {
                    Ok((recipient_id, sender)) => {
                        (ChatResponse::Message(sender, msg), recipient_id)
                    }
                    Err(err) => (ChatResponse::MessageFailure(err), initiator_id),
                };

                return Ok(AssembledResponse {
                    data: to_allocvec(&response).map_err(ProcessError::Serialize)?,
                    dest,
                });
            }
            ChatRequest::Register(username, password) => {
                let response = match self.register(username.clone(), password, initiator_id) {
                    Ok(()) => ChatResponse::ClientList(username, self.get_client_list()),
                    Err(err) => ChatResponse::RegisterFailure(username, err),
                };

                return Ok(AssembledResponse {
                    data: to_allocvec(&response).map_err(ProcessError::Serialize)?,
                    dest: initiator_id,
                });
            }
            ChatRequest::Login(username, password) => {
                let response = match self.login(&username, &password) {
                    Ok(()) => ChatResponse::ClientList(username, self.get_client_list()),
                    Err(err) => ChatResponse::LoginFailure(username, err),
                };

                return Ok(AssembledResponse {
                    data: to_allocvec(&response).map_err(ProcessError::Serialize)?,
                    dest: initiator_id,
                });
            }
            ChatRequest::Logout(username) => {
                let response = match self.logout(&username, initiator_id) {
                    Ok(()) => ChatResponse::LogoutSuccess(username),
                    Err(err) => ChatResponse::LogoutFailure(username, err),
                };

                return Ok(AssembledResponse {
                    data: to_allocvec(&response).map_err(ProcessError::Serialize)?,
                    dest: initiator_id,
                });
            }
        }
    }
}
