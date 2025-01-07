use crate::specialized_behavior::SpecializedBehavior;
use std::path::PathBuf;

pub struct MediaBehavior {
    path: PathBuf,
}

impl MediaBehavior {
    pub fn new() -> Self {
        Self {
            path: PathBuf::new(),
        }
    }
}

impl SpecializedBehavior for MediaBehavior {
    fn set_path(&mut self, path: PathBuf) -> Result<(), &str> {
        self.path = path;
        Ok(())
    }
}
