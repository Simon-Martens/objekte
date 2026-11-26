//! Repository-local source registration and collection resolution.

use std::collections::HashMap;
use std::sync::Arc;

use super::{SourceFormat, SourceReader, verification::ScalarVerification};

pub struct SourceRegistration {
    pub id: String,
    pub version: u32,
    pub extension: String,
    pub reader: Arc<dyn SourceReader>,
    file_batches: bool,
}

/// Prepared source handle resolved once per collection.
#[derive(Clone)]
pub struct PreparedSource(Arc<SourceRegistration>);

impl PreparedSource {
    pub fn id(&self) -> &str {
        &self.0.id
    }

    pub fn version(&self) -> u32 {
        self.0.version
    }

    pub fn extension(&self) -> &str {
        &self.0.extension
    }

    pub fn reader(&self) -> &dyn SourceReader {
        self.0.reader.as_ref()
    }

    pub(crate) fn reader_handle(&self) -> Arc<dyn SourceReader> {
        Arc::clone(&self.0.reader)
    }

    pub(crate) fn schedules_separate_file_jobs(&self) -> bool {
        !self.0.file_batches
    }

    pub(crate) fn scalar_verification(&self) -> Option<ScalarVerification> {
        self.reader().scalar_verification()
    }
}

/// Repository-local source registrations. A new registry is empty.
#[derive(Clone, Default)]
pub struct SourceRegistry {
    entries: HashMap<String, Arc<SourceRegistration>>,
}

impl SourceRegistry {
    pub fn register(
        &mut self,
        id: impl Into<String>,
        version: u32,
        extension: impl Into<String>,
        reader: Arc<dyn SourceReader>,
    ) -> Result<(), String> {
        let file_batches = reader.prefers_file_batches();
        self.register_source(id.into(), version, extension.into(), reader, file_batches)
    }

    /// Opt a reader into ordinary small-file batches. Its `maximum_records`
    /// implementation must bound expansion for all inputs.
    pub fn register_file_batched(
        &mut self,
        id: impl Into<String>,
        version: u32,
        extension: impl Into<String>,
        reader: Arc<dyn SourceReader>,
    ) -> Result<(), String> {
        self.register_source(id.into(), version, extension.into(), reader, true)
    }

    fn register_source(
        &mut self,
        id: String,
        version: u32,
        extension: String,
        reader: Arc<dyn SourceReader>,
        file_batches: bool,
    ) -> Result<(), String> {
        if id.is_empty() || version == 0 || extension.is_empty() || extension.contains(['/', '\\'])
        {
            return Err("invalid source registration".into());
        }
        if self.entries.contains_key(&id) {
            return Err(format!("duplicate source registration: {id}"));
        }
        self.entries.insert(
            id.clone(),
            Arc::new(SourceRegistration {
                id,
                version,
                extension,
                reader,
                file_batches,
            }),
        );
        Ok(())
    }

    pub fn resolve(&self, format: &SourceFormat) -> Result<PreparedSource, String> {
        self.entries
            .get(format.id())
            .cloned()
            .map(PreparedSource)
            .ok_or_else(|| format!("unknown source registration: {}", format.id()))
    }
}

#[cfg(test)]
mod tests {
    super::super::tests::registry_tests!();
}
