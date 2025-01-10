use crate::{fragment_manager::ToBeSentFragment, specialized_behavior::AssembledResponse};
use rust_roveri_api::SessionId;
use wg_2024::packet::{Fragment, FRAGMENT_DSIZE};

pub struct Fragmenter {
    session_id: SessionId,
}

impl Fragmenter {
    pub fn new() -> Self {
        Self { session_id: 1 }
    }

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
