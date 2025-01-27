use crate::specialized_behavior::SpecializedBehavior;

trait ContentBehavior: SpecializedBehavior {
    fn set_path(&mut self, path: PathBuf) -> Result<(), SetPathError> {
        let _ = fs::read_dir(&path).map_err(SetPathError::FileSystem)?;

        self.path = path;
        Ok(())
    }
}
