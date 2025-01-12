//! Provides functionality to fragment large messages into smaller fragment for network transmission.

use crate::{fragment_manager::ToBeSentFragment, specialized_behavior::AssembledResponse};
use rust_roveri_api::SessionId;
use wg_2024::packet::{Fragment, FRAGMENT_DSIZE};

pub struct Fragmenter {
    session_id: SessionId,
}

/// The `Fragmenter` takes an `AssembledResponse`, splits its data into smaller chunks and attach
/// the session id that is incremented after each fragmentation
impl Fragmenter {
    pub fn new() -> Self {
        Self { session_id: 1 }
    }

    /// Splits an `AssembledResponse` into a vector of fragments.
    ///
    /// This method divides the data in the `AssembledResponse` into chunks of size `FRAGMENT_DSIZE`.
    /// Each chunk is encapsulated in a `Fragment`, which is then wrapped in a `ToBeSentFragment`.
    ///
    /// # Arguments
    ///
    /// * `assembled_response` - The response data and destination to be fragmented.
    ///
    /// # Returns
    ///
    /// A `Vec<ToBeSentFragment>` containing all fragments with data for transmission.
    ///
    /// # Behavior
    ///
    /// - If the data in the `AssembledResponse` is empty, no fragments are created.
    /// - The session ID is incremented after fragmenting the data.
    pub fn to_fragment_vec(
        &mut self,
        assembled_response: AssembledResponse,
    ) -> Vec<ToBeSentFragment> {
        let data = assembled_response.data;
        let dest = assembled_response.dest;

        // Number of fragments
        let total_fragments = if data.is_empty() {
            0
        } else {
            (data.len() + FRAGMENT_DSIZE - 1) / FRAGMENT_DSIZE
        };

        let mut fragments = Vec::with_capacity(total_fragments);

        for i in 0..total_fragments {
            let start = i * FRAGMENT_DSIZE;
            let end = usize::min(start + FRAGMENT_DSIZE, data.len());
            let slice = &data[start..end];

            let mut fragment_data = [0u8; FRAGMENT_DSIZE];
            fragment_data[..slice.len()].copy_from_slice(slice);

            let fragment = Fragment {
                fragment_index: i as u64,
                total_n_fragments: total_fragments as u64,
                length: slice.len() as u8,
                data: fragment_data,
            };

            let to_be_sent_fragment = ToBeSentFragment {
                dest,
                session_id: self.session_id,
                fragment,
            };

            fragments.push(to_be_sent_fragment);
        }

        self.session_id += 1;

        fragments
    }
}
