//! Provides the functionality to manage multiple Assemblers.

use crate::assembler::{Assembler, AssemblerStatus, InsertFragmentError, RetrieveError};
use rust_roveri_api::SessionId;
use std::collections::{hash_map::Entry, HashMap};
use wg_2024::packet::Fragment;

/// Manages the assembly of fragmented data for multiple sessions.
///
/// # Overview
///
/// The `Manager` is responsible for handling the lifecycle of fragmented data across multiple
/// sessions. It provides methods to:
/// - Insert new fragments into the appropriate session.
/// - Retrieve assembled data for a session once all fragments have been received.
/// - Track the status of the assembly process for each session.
///
/// Each session is identified by a unique `SessionId`, and the manager internally uses
/// an `assembly_buffer` (HashMap) to store the fragments for each session.
pub struct AssemblersManager {
    assembly_buffer: HashMap<SessionId, Assembler>,
}

impl AssemblersManager {
    pub fn new() -> Self {
        Self {
            assembly_buffer: HashMap::new(),
        }
    }

    /// Inserts a fragment into the assembly buffer for a given session.
    ///
    /// # Arguments
    ///
    /// * `fragment` - The fragment to be inserted.
    /// * `session_id` - The identifier of the session to which the fragment belongs.
    ///
    /// # Returns
    ///
    /// - `Ok(AssemblerStatus)` if the fragment was successfully inserted into the assembly buffer. The
    ///   `AssemblerStatus` indicates the current state of the assembly process (e.g., whether the data
    ///   is fully assembled or still in progress).
    /// - `Err(InsertFragmentError)` if an error occurred during insertion. This can happen if the fragment
    ///   has a different total_n_fragments than the ones in the buffer or the fragment_index is
    ///   out of bounds.
    ///
    /// # Behavior
    ///
    /// This function ensures that the session is represented in the `assembly_buffer`. If the session
    /// does not already exist, it creates a new entry using the total number of fragments in the
    /// provided `fragment`. Then, the fragment is inserted into the corresponding assembler for the
    /// session.
    ///
    /// If the insertion is successful, the function returns the updated status of the assembler. If
    /// there are errors, they are propagated as `InsertFragmentError`.
    pub fn insert_fragment(
        &mut self,
        fragment: Fragment,
        session_id: SessionId,
    ) -> Result<AssemblerStatus, InsertFragmentError> {
        let total_fragments = fragment.total_n_fragments as usize;

        //Get the entry or create a new entry
        let entry = self
            .assembly_buffer
            .entry(session_id)
            .or_insert_with(|| Assembler::new(total_fragments));

        entry.insert_fragment(fragment)
    }

    /// Retrieves the assembled data for a given session and removes it from the buffer.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The identifier of the session for which the assembled data is retrieved.
    ///
    /// # Returns
    ///
    /// - `Ok(Vec<u8>)` if the data for the session is complete and successfully retrieved.
    /// - `Err(RetrieveError::Incomplete)` if the data is incomplete and cannot be retrieved yet.
    /// - `Err(RetrieveError::UnknownSessionId)` if the given session ID does not exist.
    ///
    /// # Behavior
    ///
    /// This function checks if the session exists in the `assembly_buffer`. If the data is complete,
    /// it removes the session from the buffer and returns the assembled data. If the data is not
    /// complete, the session remains in the buffer, and an error is returned.
    pub fn retrieve_assembled(&mut self, session_id: SessionId) -> Result<Vec<u8>, RetrieveError> {
        match self.assembly_buffer.entry(session_id) {
            Entry::Occupied(entry) => {
                let fragment_buffer = entry.get();
                if fragment_buffer.is_complete() {
                    Ok(entry.remove().retrieve_assembled()?)
                } else {
                    Err(RetrieveError::Incomplete)
                }
            }
            Entry::Vacant(_) => Err(RetrieveError::UnknownSessionId),
        }
    }
}
