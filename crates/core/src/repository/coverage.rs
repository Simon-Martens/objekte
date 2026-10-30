//! Cache status and collection coverage reporting.

use super::*;


impl Repository {
    pub fn cache_status(&mut self) -> Result<CacheStatus> {
        self.reload()?;
        let mut collections = Vec::new();
        for name in self.model.collections.keys() {
            collections.push(self.coverage(name)?);
        }
        Ok(CacheStatus {
            mode: self.options.cache,
            freshness: self.options.freshness,
            collections,
            configuration_changes: self.configuration_changes.clone(),
        })
    }

    pub(super) fn coverage(&self, name: &str) -> Result<Coverage> {
        let state = self
            .memory_coverage
            .get(name)
            .cloned()
            .unwrap_or_else(|| "absent".into());
        Ok(Coverage {
            collection: name.into(),
            verified: state == "current",
            configuration_current: state == "current",
            expected_schema_hash: self.model.collection(name)?.schema_hash.clone(),
            state,
            records: self.file_pickers.get(name).map_or_else(
                || self.memory_edges.keys().filter(|(n, _)| n == name).count(),
                crate::file_picker::FilePicker::len,
            ),
        })
    }
}
