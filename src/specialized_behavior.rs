use std::path::PathBuf;

pub trait SpecializedBehavior {
    fn set_path(&mut self, _: PathBuf) -> Result<(), &str> {
        Err("Unexpected: this server type does not support setting a path")
    }
}
