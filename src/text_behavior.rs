use crate::specialized_behavior::SpecializedBehavior;
use std::path::PathBuf;

pub struct TextBehavior {
    path: PathBuf,
}

impl TextBehavior {
    pub fn new() -> Self {
        Self {
            path: PathBuf::new(),
        }
    }
}

impl SpecializedBehavior for TextBehavior {
    fn set_path(&mut self, path: PathBuf) -> Result<(), &str> {
        self.path = path;
        Ok(())
    }
}
