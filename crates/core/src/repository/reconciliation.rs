//! Source refresh.

use super::*;


impl Repository {
    pub fn refresh(&mut self, collection: &str) -> Result<Metrics> {
        self.reload()?;
        let before = self.metrics.clone();
        let c = self.collection(collection)?;
        match self.options.cache {
            CacheMode::Segments => self.sync_segments(&c, true)?,
            CacheMode::Memory => {
                if let Some(picker) = self.file_pickers.get_mut(collection) {
                    picker.refresh()?;
                } else {
                    self.ensure_picker(&c)?;
                }
                self.memory_coverage
                    .insert(c.name.clone(), "current".into());
            }
            CacheMode::Disabled => {
                self.direct(&c, None, false)?;
            }
        }
        if self.options.cache != CacheMode::Memory
            && let Some(picker) = self.file_pickers.get_mut(collection)
        {
            picker.refresh()?;
        }
        Ok(self.metrics.since(&before))
    }
}
