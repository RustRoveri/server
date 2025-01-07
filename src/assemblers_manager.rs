use rust_roveri_api::SessionId;
use std::collections::HashMap;
use wg_2024::packet::Fragment;

use crate::fragment_buffer::{
    FragmentBuffer, FragmentBufferStatus, InsertFragmentError, RetrieveError,
};

pub struct AssemblersManager {
    assembly_buffer: HashMap<SessionId, FragmentBuffer>,
}

impl AssemblersManager {
    pub fn new() -> Self {
        Self {
            assembly_buffer: HashMap::new(),
        }
    }

    pub fn insert_fragment(
        &mut self,
        fragment: Fragment,
        session_id: SessionId,
    ) -> Result<FragmentBufferStatus, InsertFragmentError> {
        let total_fragments = fragment.total_n_fragments as usize;

        //get the entry or create a new entry
        let entry = self
            .assembly_buffer
            .entry(session_id)
            .or_insert_with(|| FragmentBuffer::new(total_fragments));

        entry.insert_fragment(fragment)
    }

    //retrieve a assembled data and remove it from the buffer
    //cost: 2 access to hashmap: O(1)
    //todo: 1 access only
    pub fn retrieve_assembled(&mut self, session_id: SessionId) -> Result<String, RetrieveError> {
        if let Some(fragment_buffer) = self.assembly_buffer.get(&session_id) {
            if !fragment_buffer.is_complete() {
                return Err(RetrieveError::Incomplete);
            }
        } else {
            return Err(RetrieveError::UnknownSessionId);
        }

        let fragment_buffer = self.assembly_buffer.remove(&session_id).unwrap();

        Ok(fragment_buffer.retrieve_assembled())
    }
}
