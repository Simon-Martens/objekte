//! Repository-local source registration and collection resolution.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::SourceFormat;
use super::SourceReader;
use super::json::{JsonDecodedScalarsReader, JsonReader};
use super::json_lines::JsonLinesReader;
use super::verification::ScalarVerification;

/// Stable source identity, discovery suffix, and reader implementation.
#[derive(Clone)]
pub struct SourceRegistration {
    pub id: String,
    pub version: u32,
    pub extension: String,
    pub reader: Arc<dyn SourceReader>,
}

/// Prepared source handle resolved once per collection.
#[derive(Clone)]
pub struct PreparedSource(Arc<SourceRegistration>, SourceProvenance);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceProvenance {
    Extension,
    Builtin(ScalarVerification),
}

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

    pub(crate) fn is_extension(&self) -> bool {
        self.1 == SourceProvenance::Extension
    }

    pub(crate) fn scalar_verification(&self) -> Option<ScalarVerification> {
        match self.1 {
            SourceProvenance::Extension => None,
            SourceProvenance::Builtin(verification) => Some(verification),
        }
    }
}

/// Repository-local source registrations. The built-ins use this same table.
#[derive(Clone, Default)]
pub struct SourceRegistry {
    entries: HashMap<String, Arc<SourceRegistration>>,
    builtin_ids: HashSet<String>,
}

impl SourceRegistry {
    pub fn builtin() -> Self {
        let mut sources = Self::default();
        sources
            .register("json", 1, "json", Arc::new(JsonReader))
            .expect("valid JSON source registration");
        sources
            .register(
                "json_decoded_scalars",
                1,
                "json",
                Arc::new(JsonDecodedScalarsReader),
            )
            .expect("valid decoded-scalar JSON source registration");
        sources
            .register("jsonl", 1, "jsonl", Arc::new(JsonLinesReader))
            .expect("valid JSON Lines source registration");
        sources
            .builtin_ids
            .extend(["json".into(), "json_decoded_scalars".into(), "jsonl".into()]);
        sources
    }

    pub fn register(
        &mut self,
        id: impl Into<String>,
        version: u32,
        extension: impl Into<String>,
        reader: Arc<dyn SourceReader>,
    ) -> Result<(), String> {
        let id = id.into();
        let extension = extension.into();
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
            }),
        );
        Ok(())
    }

    pub fn resolve(&self, format: &SourceFormat) -> Result<PreparedSource, String> {
        self.entries
            .get(format.id())
            .cloned()
            .map(|source| {
                let provenance = if self.builtin_ids.contains(&source.id) {
                    match source.id.as_str() {
                        "json" | "jsonl" => {
                            SourceProvenance::Builtin(ScalarVerification::ResolvedStrings)
                        }
                        "json_decoded_scalars" => {
                            SourceProvenance::Builtin(ScalarVerification::DecodedScalars)
                        }
                        _ => SourceProvenance::Extension,
                    }
                } else {
                    SourceProvenance::Extension
                };
                PreparedSource(source, provenance)
            })
            .ok_or_else(|| format!("unknown source registration: {}", format.id()))
    }
}

#[cfg(test)]
mod tests;
