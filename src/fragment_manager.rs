//! Manages fragments to be sent over the network.

use rust_roveri_api::{FragmentId, SessionId};
use std::collections::{HashMap, VecDeque};
use wg_2024::{network::NodeId, packet::Fragment};

/// Represents a fragment that is queued to be sent to a specific destination.
#[derive(Clone, Debug)]
pub struct ToBeSentFragment {
    pub dest: NodeId,
    pub session_id: SessionId,
    pub fragment: Fragment,
}

/// The `FragmentManager` struct is responsible for handling the storage and processing of
/// fragments that are queued to be sent. It includes a caching mechanism for retrieval
/// and a buffer for processing fragments in order.
pub struct FragmentManager {
    cache: HashMap<FragmentId, ToBeSentFragment>,
    buffer: VecDeque<ToBeSentFragment>,
}

impl FragmentManager {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            buffer: VecDeque::new(),
        }
    }

    /// Inserts a single fragment into the manager.
    ///
    /// The fragment is added to both the cache (for quick lookup) and the buffer (for processing).
    ///
    /// # Arguments
    ///
    /// * `to_be_sent_fragment` - The fragment to insert.
    pub fn insert_fragment(&mut self, to_be_sent_fragment: ToBeSentFragment) {
        self.cache.insert(
            (
                to_be_sent_fragment.session_id,
                to_be_sent_fragment.fragment.fragment_index,
            ),
            to_be_sent_fragment.clone(),
        );

        self.buffer.push_back(to_be_sent_fragment);
    }

    /// Pop a frugment from the buffer
    ///
    /// # Returns
    ///
    /// - `Some(ToBeSentFragment)` if the buffer is not empty.
    /// - `None` if the buffer is empty.
    pub fn get_next(&mut self) -> Option<ToBeSentFragment> {
        self.buffer.pop_front()
    }

    /// Inserts multiple fragments into the manager in bulk.
    ///
    /// # Arguments
    ///
    /// * `fragments` - A vector of fragments to insert.
    pub fn insert_bulk(&mut self, fragments: Vec<ToBeSentFragment>) {
        for fragment in fragments.into_iter() {
            self.insert_fragment(fragment);
        }
    }

    /// Re-inserts a fragment from the cache back into the buffer.
    ///
    /// # Arguments
    ///
    /// * `fragment_id` - The ID of the fragment to re-insert.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the fragment is found in the cache and successfully re-inserted.
    /// - `Err(&str)` if the fragment is not found in the cache.
    pub fn insert_from_cache(&mut self, fragment_id: FragmentId) -> Result<(), &str> {
        let to_be_sent_fragment = match self.cache.get(&fragment_id) {
            None => return Err("Requested fragment is not in cache"),
            Some(fragment) => fragment.clone(),
        };

        self.buffer.push_back(to_be_sent_fragment);

        Ok(())
    }

    /// Removes a fragment from the cache.
    ///
    /// # Arguments
    ///
    /// * `fragment_id` - The ID of the fragment to remove.
    pub fn remove_from_cache(&mut self, fragment_id: FragmentId) {
        self.cache.remove(&fragment_id);
    }
}
