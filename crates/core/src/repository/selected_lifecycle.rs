//! Ordinary lifecycle glue for selected-source postings. Native JSON caches
//! retain their existing format and concrete picker path.

use super::*;
use crate::{repository::extension_projection::SelectedIndexRequest, text::TextRules};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{fs, io::Write as _, path::PathBuf};

const MANIFEST_VERSION: u32 = 2;


#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) enum SelectedSpec {
    Text {
        representation: String,
        path: String,
    },
    Derived {
        extractor: String,
    },
    Field {
        path: String,
    },
}


#[derive(Serialize, Deserialize)]
struct SelectedManifest {
    version: u32,
    specs: Vec<SelectedSpec>,
}


impl Repository {
    fn selected_cache_file(&self, collection: &str, spec: Option<&SelectedSpec>) -> PathBuf {
        let digest = Sha256::digest(
            serde_json::to_vec(&(collection, spec)).expect("cache identity serializes"),
        );
        let kind = if spec.is_some() {
            "posting"
        } else {
            "inventory"
        };
        self.model
            .root
            .join(".objekte/.cache")
            .join(format!("selected-{kind}-{digest:x}.bin"))
    }

    fn selected_auto_text_specs(&self, collection: &Collection) -> Vec<SelectedSpec> {
        let mut specs = BTreeSet::new();
        for (path, format) in TextRules::new(&collection.fields).declared_text_paths() {
            let visible = match format {
                crate::TextFormat::Plain => "objekte:visible-plain",
                crate::TextFormat::Markdown => "objekte:visible-markdown",
                crate::TextFormat::Html => "objekte:visible-html",
                crate::TextFormat::Xml => "objekte:visible-xml",
            };
            specs.insert(SelectedSpec::Text {
                representation: "objekte:stored".into(),
                path: path.clone(),
            });
            specs.insert(SelectedSpec::Text {
                representation: visible.into(),
                path,
            });
        }
        specs.into_iter().collect()
    }

    fn selected_auto_field_specs(&self, collection: &Collection) -> Vec<SelectedSpec> {
        let mut specs = BTreeSet::new();
        for index in &collection.indexes {
            specs.insert(SelectedSpec::Field {
                path: crate::json_filter::format_path(&vec![crate::JsonPathStep::Key(
                    index.name.clone(),
                )]),
            });
        }
        for path in &collection.advanced_index_paths {
            specs.insert(SelectedSpec::Field {
                path: crate::json_filter::format_path(path),
            });
        }
        specs.into_iter().collect()
    }

    pub(super) fn active_selected_specs(&self, collection: &str) -> Result<Vec<SelectedSpec>> {
        let Some(snapshot) = self.custom_source_snapshot.get(collection) else {
            return Ok(Vec::new());
        };
        self.selected_specs_for(collection, &snapshot.fingerprint)
    }

    fn selected_specs_for(&self, collection: &str, selection: &str) -> Result<Vec<SelectedSpec>> {
        let mut specs = BTreeSet::new();
        for key in self.extension_text_indexes.keys() {
            if key.collection == collection && key.selection == selection {
                specs.insert(SelectedSpec::Text {
                    representation: key.representation.clone(),
                    path: key.path.clone(),
                });
            }
        }
        for key in self.selected_derived_indexes.keys() {
            if key.collection == collection
                && key.selection == selection
                && !crate::extensions::is_derived_path(&key.path)
            {
                specs.insert(SelectedSpec::Field {
                    path: crate::json_filter::format_path(&key.path),
                });
            }
        }
        if let Some(extensions) = &self.extensions {
            for identity in extensions.extractor_identities() {
                let Ok(binding) = extensions.bind_value(
                    identity,
                    collection,
                    extensions.schema(collection).map(std::sync::Arc::as_ref),
                ) else {
                    continue;
                };
                if self.selected_derived_indexes.keys().any(|key| {
                    key.collection == collection
                        && key.selection == selection
                        && key.path == binding.path
                }) {
                    specs.insert(SelectedSpec::Derived {
                        extractor: identity.clone(),
                    });
                }
            }
        }
        Ok(specs.into_iter().collect())
    }

    fn restore_selected_spec(&mut self, collection: &str, spec: &SelectedSpec) -> Result<bool> {
        let path = self.selected_cache_file(collection, Some(spec));
        match spec {
            SelectedSpec::Text {
                representation,
                path: field,
            } => self.restore_selected_representation_snapshot(
                collection,
                representation,
                field,
                &path,
            ),
            SelectedSpec::Derived { extractor } => {
                self.restore_selected_derived_snapshot(collection, extractor, &path)
            }
            SelectedSpec::Field { path: field } => {
                self.restore_selected_field_snapshot(collection, field, &path)
            }
        }
    }

    pub(super) fn build_selected_specs(
        &mut self,
        collection: &str,
        specs: &[SelectedSpec],
    ) -> Result<()> {
        self.build_selected_specs_reusing(collection, specs, None)
    }

    pub(super) fn build_selected_specs_reusing(
        &mut self,
        collection: &str,
        specs: &[SelectedSpec],
        previous: Option<&CustomSourceSnapshot>,
    ) -> Result<()> {
        let requests = specs
            .iter()
            .map(|spec| match spec {
                SelectedSpec::Text {
                    representation,
                    path,
                } => SelectedIndexRequest::Text {
                    representation,
                    path,
                },
                SelectedSpec::Derived { extractor } => SelectedIndexRequest::Derived { extractor },
                SelectedSpec::Field { path } => SelectedIndexRequest::Field { path },
            })
            .collect::<Vec<_>>();
        if !requests.is_empty() {
            self.build_selected_indexes_reusing(collection, &requests, previous)?;
        }
        Ok(())
    }

    /// Restore compatible postings from the ordinary repository cache and
    /// rebuild only requested outputs that could not be restored.
    pub(super) fn warm_selected_indexes(
        &mut self,
        collection: &str,
        include_text: bool,
        include_derived: bool,
    ) -> Result<()> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return Ok(());
        }
        let selected_collection = self.model.collection(collection)?.clone();
        let path = self.selected_cache_file(collection, None);
        let saved = fs::File::open(&path).ok().and_then(|file| {
            let mut bytes = Vec::new();
            file.take(1_048_577).read_to_end(&mut bytes).ok()?;
            (bytes.len() <= 1_048_576)
                .then(|| {
                    serde_json::from_slice::<SelectedManifest>(&bytes)
                        .ok()
                        .filter(|manifest| {
                            manifest.version == MANIFEST_VERSION && manifest.specs.len() <= 4096
                        })
                })
                .flatten()
        });
        let mut specs = saved.map_or_else(Vec::new, |manifest| manifest.specs);
        if !include_derived {
            specs.retain(|spec| matches!(spec, SelectedSpec::Text { .. }));
        }
        let auto = if include_text {
            self.selected_auto_text_specs(&selected_collection)
        } else {
            Vec::new()
        };
        specs.extend(auto.iter().cloned());
        if include_derived {
            specs.extend(self.selected_auto_field_specs(&selected_collection));
        }
        specs.sort();
        specs.dedup();
        let mut missing = Vec::new();
        for spec in specs {
            match self.restore_selected_spec(collection, &spec) {
                Ok(true) => {}
                Ok(false) => missing.push(spec),
                Err(Error::Request(_)) if !auto.contains(&spec) => {}
                Err(error) => return Err(error),
            }
        }
        self.build_selected_specs(collection, &missing)?;
        Ok(())
    }

    pub(super) fn rebuild_selected_text_indexes(&mut self, collection: &str) -> Result<()> {
        let selected_collection = self.model.collection(collection)?.clone();
        let mut specs = self.selected_auto_text_specs(&selected_collection);
        specs.extend(
            self.active_selected_specs(collection)?
                .into_iter()
                .filter(|spec| matches!(spec, SelectedSpec::Text { .. })),
        );
        specs.sort();
        specs.dedup();
        self.extension_text_indexes
            .retain(|key, _| key.collection != collection);
        self.build_selected_specs(collection, &specs)
    }

    pub(super) fn warm_selected_text_field(
        &mut self,
        collection: &str,
        representation: &str,
        field: &str,
    ) -> Result<()> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return Ok(());
        }
        if self.options.freshness == Freshness::MetadataOnOpen
            && let (Some(snapshot), Some(extensions)) = (
                self.custom_source_snapshot.get(collection),
                self.extensions.as_ref(),
            )
        {
            let selected_collection = self.model.collection(collection)?;
            let key = crate::repository::extension_index::RepresentationIndexKey::new(
                selected_collection,
                field,
                representation,
                &snapshot.fingerprint,
                extensions,
                selected_collection
                    .bigram_encoding
                    .unwrap_or(self.options.bigram_encoding),
            );
            if self.extension_text_indexes.contains_key(&key) {
                return Ok(());
            }
        }
        let spec = SelectedSpec::Text {
            representation: representation.into(),
            path: field.into(),
        };
        if !self.restore_selected_spec(collection, &spec)? {
            self.build_selected_specs(collection, &[spec])?;
        }
        Ok(())
    }

    pub(super) fn warm_selected_derived(
        &mut self,
        collection: &str,
        extractor: &str,
    ) -> Result<()> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return Ok(());
        }
        if self.options.freshness == Freshness::MetadataOnOpen
            && let (Some(snapshot), Some(extensions)) = (
                self.custom_source_snapshot.get(collection),
                self.extensions.as_ref(),
            )
        {
            let selected_collection = self.model.collection(collection)?;
            let binding = extensions.bind_value(
                extractor,
                collection,
                extensions.schema(collection).map(std::sync::Arc::as_ref),
            )?;
            let key = crate::repository::extension_derived_index::SelectedDerivedKey::new(
                selected_collection,
                &binding.path,
                &snapshot.fingerprint,
                extensions,
            );
            if self.selected_derived_indexes.contains_key(&key) {
                return Ok(());
            }
        }
        let spec = SelectedSpec::Derived {
            extractor: extractor.into(),
        };
        if !self.restore_selected_spec(collection, &spec)? {
            self.build_selected_specs(collection, &[spec])?;
        }
        Ok(())
    }

    pub(super) fn save_selected_indexes(&self, collection: &str) -> Result<()> {
        let specs = if self.options.freshness == Freshness::MetadataOnOpen {
            self.active_selected_specs(collection)?
        } else if let Some(extensions) = &self.extensions {
            let sources = self.source_descriptors(collection)?;
            let selection = extensions
                .selection_dependencies(self.model.collection(collection)?, &sources)?
                .0;
            self.selected_specs_for(collection, &selection)?
        } else {
            Vec::new()
        };
        let path = self.selected_cache_file(collection, None);
        for spec in &specs {
            let target = self.selected_cache_file(collection, Some(spec));
            match spec {
                SelectedSpec::Text {
                    representation,
                    path,
                } => self.save_selected_representation_snapshot(
                    collection,
                    representation,
                    path,
                    &target,
                )?,
                SelectedSpec::Derived { extractor } => {
                    self.save_selected_derived_snapshot(collection, extractor, &target)?
                }
                SelectedSpec::Field { path } => {
                    self.save_selected_field_snapshot(collection, path, &target)?
                }
            }
        }
        let bytes = serde_json::to_vec(&SelectedManifest {
            version: MANIFEST_VERSION,
            specs,
        })
        .map_err(|error| Error::Cache(format!("encode selected manifest: {error}")))?;
        let parent = path.parent().expect("selected cache has parent");
        fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| io(parent, error))?;
        temporary
            .write_all(&bytes)
            .map_err(|error| io(&path, error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| io(&path, error))?;
        temporary
            .persist(&path)
            .map_err(|error| io(&path, error.error))?;
        Ok(())
    }
}
