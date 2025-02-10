//! Provides the functionality to assemble fragments of data into a complete set (`Vec<u8>`).

use wg_2024::packet::{Fragment, FRAGMENT_DSIZE};

/// Represents the status of the assembler after a fragment is inserted.
pub enum AssemblerStatus {
    Complete,
    Incomplete,
}

/// Errors that can occur while inserting a fragment into the assembler.
pub enum InsertFragmentError {
    CapacityDoesNotMatch,
    IndexOutOfBounds,
}

/// Errors that can occur while retrieving assembled data.
pub enum RetrieveError {
    Incomplete,
    UnknownSessionId,
}

/// Responsible for assembling data fragments into a complete set.
///
/// The `Assembler` maintains an internal buffer to store fragments and track the
/// assembly process. Fragments are inserted one by one, and the assembler checks for
/// completeness after each insertion.
pub struct Assembler {
    data: Vec<Option<[u8; FRAGMENT_DSIZE]>>,
    fragments_left: usize,
}

impl Assembler {
    pub fn new(total_fragments: usize) -> Self {
        Self {
            data: vec![None; total_fragments],
            fragments_left: total_fragments,
        }
    }

    /// Checks whether the assembly is complete.
    pub fn is_complete(&self) -> bool {
        self.fragments_left == 0
    }

    /// Inserts a fragment into the assembler.
    ///
    /// # Arguments
    ///
    /// * `fragment` - The fragment to be inserted.
    ///
    /// # Returns
    ///
    /// - `Ok(AssemblerStatus::Complete)` if the assembly is complete after the insertion.
    /// - `Ok(AssemblerStatus::Incomplete)` if more fragments are needed to complete the assembly.
    /// - `Err(InsertFragmentError::CapacityDoesNotMatch)` if the total_n_fragments field does not match
    ///   the assembler's capacity.
    /// - `Err(InsertFragmentError::IndexOutOfBounds)` if the fragment's index is invalid.
    pub fn insert_fragment(
        &mut self,
        fragment: Fragment,
    ) -> Result<AssemblerStatus, InsertFragmentError> {
        let index = fragment.fragment_index as usize;
        let total_fragments = fragment.total_n_fragments as usize;

        //Check if vec size matches
        if total_fragments != self.data.len() {
            return Err(InsertFragmentError::CapacityDoesNotMatch);
        }

        //Check if index is in bounds
        if index >= self.data.len() {
            return Err(InsertFragmentError::IndexOutOfBounds);
        }

        //Check if we are replacing or inserting the data -> decrementing fragments left number
        if self.data[index].is_none() {
            self.fragments_left -= 1;
        }

        self.data[index] = Some(fragment.data);

        if self.is_complete() {
            Ok(AssemblerStatus::Complete)
        } else {
            Ok(AssemblerStatus::Incomplete)
        }
    }

    /// Retrieves the assembled data as a single continuous byte vector.
    ///
    /// # Returns
    ///
    /// - `Ok(Vec<u8>)` if all fragments are present and the data is successfully assembled.
    /// - `Err(RetrieveError::Incomplete)` if one or more fragments are missing.
    ///
    /// # Behavior
    ///
    /// The function checks if all fragments are present. If any fragment is missing, it returns
    /// an error. If all fragments are present, it concatenates them and
    /// returns the resulting byte vector.
    pub fn retrieve_assembled(self) -> Result<Vec<u8>, RetrieveError> {
        let mut assembled = Vec::with_capacity(self.data.len() * FRAGMENT_DSIZE);

        for fragment in self.data {
            match fragment {
                Some(data) => assembled.extend_from_slice(&data),
                None => return Err(RetrieveError::Incomplete),
            }
        }

        Ok(assembled)
    }
}
