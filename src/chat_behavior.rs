//! Implements the `ChatBehavior` struct for managing chat clients and handling requests.

use crate::specialized_behavior::{AssembledResponse, ProcessError, SpecializedBehavior};
use log::error;
use postcard::{from_bytes, to_allocvec};
use rust_roveri_api::{
    ChatRequest, ChatResponse, ClientListError, LoginError, LogoutError, MessageError, Password,
    RegisterError, UserName,
};
use std::collections::{hash_map::Entry, HashMap};
use wg_2024::network::NodeId;

type Logged = bool;

pub struct ChatBehavior {
    clients: HashMap<UserName, (Password, NodeId, Logged)>,
}

/// Handles chat client management and network request processing.
///
/// The `ChatBehavior` struct keeps track of registered clients, their authentication states,
/// and their associated network identifiers.
impl ChatBehavior {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    /// Retrieves a list of all registered usernames.
    fn get_client_list(&self) -> Vec<UserName> {
        self.clients
            .iter()
            .map(|(username, _)| username.clone())
            .collect()
    }

    /// Registers a new client with the given username, password, and node ID.
    ///
    /// # Arguments
    ///
    /// * `username` - New client's username.
    /// * `password` - The password associated with the client.
    /// * `node_id` - The `NodeId` associated with the client connection.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the registration is successful.
    /// - `Err(RegisterError::AlreadyRegistered)` if the username is already in use.
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

    /// Checks if a client is authenticated and associated with a specific node ID.
    ///
    /// # Arguments
    ///
    /// * `username` - The username of the client to check.
    /// * `node_id` - The `NodeId` associated with the client's session.
    ///
    /// # Returns
    ///
    /// `true` if the client is authenticated and their `NodeId` matches; otherwise, `false`.
    fn is_auth(&self, username: &UserName, node_id: NodeId) -> bool {
        match self.clients.get(username) {
            Some((_, id, logged)) => node_id == *id && *logged,
            None => false,
        }
    }

    /// Verifies if the sender is authenticated and the recipient can receive a message.
    ///
    /// # Arguments
    ///
    /// * `sender` - The username of the message sender.
    /// * `node_id` - The `NodeId` associated with the sender's session.
    /// * `recipient` - The username of the message recipient.
    ///
    /// # Returns
    ///
    /// - `Ok((NodeId, UserName))` with the recipient's `NodeId` and the sender's username if the
    ///   message can be sent.
    /// - `Err(MessageError)` if the sender is not authenticated, the recipient is not registered,
    ///   or the recipient is not logged in.
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

    /// Signin a client if its credentials are valid.
    ///
    /// # Arguments
    ///
    /// * `username` - The username of the client.
    /// * `password` - The password provided by the client.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the login is successful.
    /// - `Err(LoginError)` if the client is already logged in, not registered, or the password is incorrect.
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

    /// Signout a client if they are currently logged in.
    ///
    /// # Arguments
    ///
    /// * `username` - The username of the client.
    /// * `node_id` - The `NodeId` associated with the client's session.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the logout is successful.
    /// - `Err(LogoutError)` if the client is not logged in, the `NodeId` does not match,
    ///   or the client is not registered.
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

    /// Processes a chat request and generates an appropriate response.
    ///
    /// # Arguments
    ///
    /// * `assembled` - The serialized chat request.
    /// * `initiator_id` - The ID of the node that sent the request.
    ///
    /// # Returns
    ///
    /// - `Ok(AssembledResponse)` if the request is successfully processed.
    /// - `Err(ProcessError)` if an error occurs during request processing.
    ///
    /// # Behavior
    ///
    /// This method supports the following types of requests:
    /// - `ClientList`: Returns the list of connected clients.
    /// - `Message`: Sends a message from one client to another.
    /// - `Register`: Registers a new client with a username and password.
    /// - `Login`: Authenticates a client and logs them in.
    /// - `Logout`: Logs a client out of the system.
    fn process_assembled(
        &mut self,
        assembled: Vec<u8>,
        initiator_id: NodeId,
    ) -> Result<AssembledResponse, ProcessError> {
        let content_request =
            from_bytes::<ChatRequest>(&assembled).map_err(ProcessError::Deserialize)?;

        match content_request {
            ChatRequest::ClientList(username) => {
                let response = if self.is_auth(&username, initiator_id) {
                    ChatResponse::ClientList(username, self.get_client_list());
                } else {
                    ChatResponse::ClientListFailure(username, ClientListError::NotLogged);
                };

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
                    Ok(_) => ChatResponse::ClientList(username, self.get_client_list()),
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
