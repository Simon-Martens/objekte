//! Frozen programmatic collection and record-source registration.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::CollectionId;
use crate::repository::configuration::{CollectionDeclaration, RepositoryOptions};
use crate::schema::Schema;

use super::{Index, IndexError};

static NEXT_BUILDER_ID: AtomicU64 = AtomicU64::new(1);

/// An index-scoped registration for one collection and stable adapter identity.
/// A registration issued by one builder cannot write into a different index.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RecordRegistration {
    pub(crate) builder_id: u64,
    pub(crate) ordinal: u32,
    pub(crate) collection: CollectionId,
}

impl RecordRegistration {
    pub fn collection(self) -> CollectionId {
        self.collection
    }
}

pub(crate) struct RegisteredRecordSource {
    pub(crate) handle: RecordRegistration,
    pub(crate) adapter_id: String,
    pub(crate) version: u32,
    pub(crate) representation_version: u32,
}

/// Declare collections and record sources before publishing the first view.
pub struct IndexBuilder {
    builder_id: u64,
    collections: Vec<CollectionDeclaration>,
    registrations: Vec<RegisteredRecordSource>,
    options: RepositoryOptions,
}

impl Default for IndexBuilder {
    fn default() -> Self {
        Self {
            builder_id: NEXT_BUILDER_ID.fetch_add(1, Ordering::Relaxed),
            collections: Vec::new(),
            registrations: Vec::new(),
            options: RepositoryOptions::default(),
        }
    }
}

impl Index {
    /// Begin a programmatic index configuration; no path or file service is needed.
    pub fn builder() -> IndexBuilder {
        IndexBuilder::default()
    }
}

impl IndexBuilder {
    pub fn with_options(mut self, options: RepositoryOptions) -> Self {
        self.options = options;
        self
    }

    /// Declare a collection whose schema and indexing policy remain fixed.
    pub fn collection(
        &mut self,
        name: impl Into<String>,
        schema: Schema,
    ) -> Result<CollectionId, IndexError> {
        self.declare_collection(CollectionDeclaration::new(name, schema))
    }

    /// Declare a collection with explicit sort and index policy.
    pub fn declare_collection(
        &mut self,
        declaration: CollectionDeclaration,
    ) -> Result<CollectionId, IndexError> {
        if declaration.name.is_empty() {
            return Err(IndexError::Build(
                "collection name must not be empty".into(),
            ));
        }
        if self
            .collections
            .iter()
            .any(|existing| existing.name == declaration.name)
        {
            return Err(IndexError::Build(format!(
                "duplicate collection: {}",
                declaration.name
            )));
        }
        let id = CollectionId(
            u32::try_from(self.collections.len())
                .map_err(|_| IndexError::Build("too many collections".into()))?,
        );
        self.collections.push(declaration);
        Ok(id)
    }

    /// Bind a stable adapter identity to a declared collection.
    pub fn register_records(
        &mut self,
        collection: CollectionId,
        adapter_id: impl Into<String>,
        version: u32,
    ) -> Result<RecordRegistration, IndexError> {
        self.register_records_with_representation(collection, adapter_id, version, 1)
    }

    pub fn register_records_with_representation(
        &mut self,
        collection: CollectionId,
        adapter_id: impl Into<String>,
        version: u32,
        representation_version: u32,
    ) -> Result<RecordRegistration, IndexError> {
        if self.collections.get(collection.0 as usize).is_none() {
            return Err(IndexError::Build(format!(
                "unknown collection: {}",
                collection.0
            )));
        }
        let adapter_id = adapter_id.into();
        if adapter_id.is_empty() {
            return Err(IndexError::Build("adapter ID must not be empty".into()));
        }
        if self.registrations.iter().any(|existing| {
            existing.handle.collection == collection
                && existing.adapter_id == adapter_id
                && existing.version == version
        }) {
            return Err(IndexError::Build(format!(
                "duplicate record registration: {adapter_id}/{version}"
            )));
        }
        let handle = RecordRegistration {
            builder_id: self.builder_id,
            ordinal: u32::try_from(self.registrations.len())
                .map_err(|_| IndexError::Build("too many record registrations".into()))?,
            collection,
        };
        self.registrations.push(RegisteredRecordSource {
            handle,
            adapter_id,
            version,
            representation_version,
        });
        Ok(handle)
    }

    /// Publish a usable empty index with no file discovery or source reads.
    pub fn build(self) -> Result<Index, IndexError> {
        let identities = self
            .registrations
            .iter()
            .map(|registration| {
                (
                    registration.handle.collection,
                    registration.adapter_id.clone(),
                    registration.version,
                    registration.representation_version,
                )
            })
            .collect::<Vec<_>>();
        let repository = crate::construction::repository_state::initialize_empty_programmatic(
            self.collections,
            self.options,
            &identities,
        )
        .map_err(IndexError::Build)?;
        let mut index = Index::from_repository(repository)?;
        Arc::get_mut(&mut index.0)
            .expect("newly constructed index has no additional owners")
            .record_registrations = self.registrations;
        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_publishes_empty_fileless_index_with_frozen_registrations() {
        let mut builder = Index::builder();
        let entries = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(entries, "application.entries", 1)
            .unwrap();
        let index = builder.build().unwrap();
        assert_eq!(index.0.record_registrations.len(), 1);
        assert_eq!(index.0.record_registrations[0].handle, registration);
        let view = index.view().unwrap();
        assert_eq!(
            view.collection_declarations().next().unwrap().name,
            "entries"
        );
        assert_eq!(view.index_observation().files, 0);
        assert_eq!(view.index_observation().records, 0);
        assert!(
            view.get_record(
                "entries",
                "missing",
                crate::query::options::ReturnMode::Locator
            )
            .unwrap()
            .is_none()
        );
    }
}
