use wg_2024::packet::{Fragment, FRAGMENT_DSIZE};

pub enum FragmentBufferStatus {
    Complete,
    Incomplete,
}

pub enum InsertFragmentError {
    CapacityDoesNotMatch,
    IndexOutOfBounds,
}

pub enum RetrieveError {
    Incomplete,
    UnknownSessionId,
}

pub struct FragmentBuffer {
    data: Vec<Option<[u8; FRAGMENT_DSIZE]>>,
    fragments_left: usize,
}

impl FragmentBuffer {
    pub fn new(total_fragments: usize) -> Self {
        Self {
            data: Vec::with_capacity(total_fragments),
            fragments_left: total_fragments,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.fragments_left == 0
    }

    pub fn insert_fragment(
        &mut self,
        fragment: Fragment,
    ) -> Result<FragmentBufferStatus, InsertFragmentError> {
        let index = fragment.fragment_index as usize;
        let total_fragments = fragment.total_n_fragments as usize;

        //check if vec size matches
        if total_fragments != self.data.len() {
            return Err(InsertFragmentError::CapacityDoesNotMatch);
        }

        //check if index is in bounds
        if index >= self.data.len() {
            return Err(InsertFragmentError::CapacityDoesNotMatch);
        }

        //check if we are replacing or inserting the data -> decrementing fragments left number
        if self.data.get(index).is_none() {
            self.fragments_left -= 1;
        }

        self.data[index] = Some(fragment.data);

        if self.is_complete() {
            Ok(FragmentBufferStatus::Complete)
        } else {
            Ok(FragmentBufferStatus::Incomplete)
        }
    }

    pub fn retrieve_assembled(self) -> String {
        let mut assembled = String::with_capacity(self.data.len() * FRAGMENT_DSIZE);

        for fragment in self.data {
            let fragment = match fragment {
                Some(fragment) => fragment,
                None => panic!("Unexpected: fragments_left is 0 but data is None"),
            };

            let fragment_as_str = match std::str::from_utf8(&fragment) {
                Ok(res) => res,
                Err(_) => panic!("Unexpected: cannot convert to char"),
            };

            assembled.push_str(fragment_as_str);
        }

        assembled
    }
}
