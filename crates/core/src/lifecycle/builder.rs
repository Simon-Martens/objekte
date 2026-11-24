//! Frozen programmatic collection and record-source registration.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::files::FileCatalog;
use crate::model::CollectionId;
use crate::repository::configuration::{
    CollectionDeclaration, FileSourceConfig, RepositoryOptions,
};
use crate::schema::Schema;
use crate::source::registry::SourceRegistry;

use super::{Index, IndexError};

static NEXT_BUILDER_ID: AtomicU64 = AtomicU64::new(1);

/// An index-scoped registration for one collection and stable adapter identity.
/// A registration issued by one builder cannot write into a different index.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordRegistration {
    pub(crate) builder_id: u64,
    pub(crate) ordinal: u32,
    pub(crate) collection: CollectionId,
}

/// Capability for the single file catalog installed on this index.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileCatalogRegistration {
    builder_id: u64,
}

impl RecordRegistration {
    pub fn collection(self) -> CollectionId {
        self.collection
    }
}

#[derive(Clone)]
pub(crate) struct RegisteredRecordSource {
    pub(crate) handle: RecordRegistration,
    pub(crate) adapter_id: String,
    pub(crate) version: u32,
    pub(crate) representation_version: u32,
    pub(crate) file_links: bool,
    pub(crate) reader: Option<std::sync::Arc<dyn crate::records::reader::RecordReader>>,
}

/// Declare collections and record sources before publishing the first view.
pub struct IndexBuilder {
    builder_id: u64,
    collections: Vec<CollectionDeclaration>,
    file_sources: Vec<Option<FileSourceConfig>>,
    registrations: Vec<RegisteredRecordSource>,
    catalog_root: Option<PathBuf>,
    sources: SourceRegistry,
    options: RepositoryOptions,
}

impl Default for IndexBuilder {
    fn default() -> Self {
        Self {
            builder_id: NEXT_BUILDER_ID.fetch_add(1, Ordering::Relaxed),
            collections: Vec::new(),
            file_sources: Vec::new(),
            registrations: Vec::new(),
            catalog_root: None,
            sources: SourceRegistry::builtin(),
            options: RepositoryOptions::default(),
        }
    }
}

impl Index {
    /// Begin a programmatic index configuration; no path or file service is needed.
    ///
    /// # Example: owned typed records
    ///
    /// ```
    /// use objekte_search_core::lifecycle::Index;
    /// use objekte_search_core::model::Value;
    /// use objekte_search_core::query::options::ReturnMode;
    /// use objekte_search_core::schema::Schema;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut builder = Index::builder();
    /// let collection = builder.collection("entries", Schema::default())?;
    /// let registration = builder.register_records(collection, "my.entries", 1)?;
    /// let index = builder.build()?;
    /// let session = index.ingestion();
    /// let writer = session.writer(registration)?;
    /// let mut batch = writer.batch();
    /// batch.insert("entry-1", Value::String("Leipzig".into()))?;
    /// writer.submit(batch).unwrap().wait_published()?;
    /// assert!(index.view()?.get_record("entries", "entry-1", ReturnMode::Logical)?.is_some());
    /// session.finish()?;
    /// # Ok(()) }
    /// ```
    pub fn builder() -> IndexBuilder {
        IndexBuilder::default()
    }

    /// Reuse the active ingestion session, or start a fresh ordered session
    /// after its predecessor has finished or failed.
    pub fn ingestion(&self) -> crate::ingestion::IngestionSession {
        if let Ok(active) = self.0.ingestion.lock()
            && let Some(core) = active.upgrade()
            && core.is_open()
        {
            return crate::ingestion::IngestionSession::from_core(core);
        }
        self.ingestion_with_options(crate::ingestion::IngestionOptions::default())
            .expect("default ingestion session could not start")
    }

    pub fn ingestion_with_options(
        &self,
        options: crate::ingestion::IngestionOptions,
    ) -> Result<crate::ingestion::IngestionSession, IndexError> {
        let mut active = self
            .0
            .ingestion
            .lock()
            .map_err(|_| IndexError::LockPoisoned)?;
        if let Some(core) = active.upgrade()
            && core.is_open()
        {
            if core.options() != &options {
                return Err(IndexError::Build(
                    "active ingestion session has different options".into(),
                ));
            }
            return Ok(crate::ingestion::IngestionSession::from_core(core));
        }
        let core = crate::ingestion::queue::SessionCore::new(self.clone(), options);
        *active = std::sync::Arc::downgrade(&core);
        Ok(crate::ingestion::IngestionSession::from_core(core))
    }
}

impl IndexBuilder {
    /// Install the readers used by any file importers declared before build.
    pub fn with_source_registry(mut self, sources: SourceRegistry) -> Self {
        self.sources = sources;
        self
    }

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
        self.file_sources.push(None);
        Ok(id)
    }

    /// Let the existing file mutation API import files into this collection.
    /// A file catalog must also be registered before `build`.
    pub fn register_file_source(
        &mut self,
        collection: CollectionId,
        source: FileSourceConfig,
    ) -> Result<(), IndexError> {
        self.sources
            .resolve(&source.format)
            .map_err(IndexError::Build)?;
        let entry = self
            .file_sources
            .get_mut(collection.0 as usize)
            .ok_or(IndexError::ForeignInstance)?;
        if entry.is_some() {
            return Err(IndexError::Build(
                "file source is already registered".into(),
            ));
        }
        *entry = Some(source);
        Ok(())
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
            file_links: false,
            reader: None,
        });
        Ok(handle)
    }

    /// Bind a versioned projected reader before freezing the index.
    ///
    /// # Example: reread one immutable version
    ///
    /// ```
    /// use std::sync::Arc;
    /// use objekte_search_core::lifecycle::Index;
    /// use objekte_search_core::model::Value;
    /// use objekte_search_core::records::{RecordKey, RecordVersion};
    /// use objekte_search_core::records::reader::{RecordProjection, RecordRead, RecordReadError, RecordReader};
    /// use objekte_search_core::schema::Schema;
    /// struct Reader;
    /// impl RecordReader for Reader {
    ///     fn read_projected(&self, _: &RecordKey, expected: RecordVersion, _: &RecordProjection)
    ///         -> Result<RecordRead, RecordReadError> {
    ///         if expected != RecordVersion::new(7) {
    ///             return Err(RecordReadError::ChangedVersion { expected, actual: Some(RecordVersion::new(7)) });
    ///         }
    ///         Ok(RecordRead { version: expected, value: Value::String("Leipzig".into()),
    ///             text: Vec::new(), original_bytes: None })
    ///     }
    /// }
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut builder = Index::builder();
    /// let collection = builder.collection("entries", Schema::default())?;
    /// let registration = builder.register_records(collection, "my.reader", 1)?;
    /// builder.register_reader(registration, Arc::new(Reader))?;
    /// let index = builder.build()?;
    /// let session = index.ingestion();
    /// let writer = session.writer(registration)?;
    /// let mut batch = writer.batch();
    /// batch.insert_from_reader("entry-1", Value::String("Leipzig".into()), RecordVersion::new(7))?;
    /// writer.submit(batch).unwrap().wait_published()?;
    /// session.finish()?;
    /// # Ok(()) }
    /// ```
    pub fn register_reader(
        &mut self,
        registration: RecordRegistration,
        reader: std::sync::Arc<dyn crate::records::reader::RecordReader>,
    ) -> Result<(), IndexError> {
        if registration.builder_id != self.builder_id {
            return Err(IndexError::ForeignInstance);
        }
        let source = self
            .registrations
            .get_mut(registration.ordinal as usize)
            .filter(|source| source.handle == registration)
            .ok_or(IndexError::ForeignInstance)?;
        if source.reader.is_some() {
            return Err(IndexError::Build("record reader already registered".into()));
        }
        source.reader = Some(reader);
        Ok(())
    }

    /// Install one file service before build. An installed empty catalog is
    /// still visible to readers as a service with zero files.
    pub fn register_file_catalog(
        &mut self,
        catalog: FileCatalog,
    ) -> Result<FileCatalogRegistration, IndexError> {
        if self.catalog_root.is_some() {
            return Err(IndexError::Build(
                "file catalog is already registered".into(),
            ));
        }
        self.catalog_root = Some(catalog.into_root());
        Ok(FileCatalogRegistration {
            builder_id: self.builder_id,
        })
    }

    /// Allow this registration to attach optional file and range provenance.
    ///
    /// # Example: two records in one file
    ///
    /// ```
    /// use objekte_search_core::files::FileCatalog;
    /// use objekte_search_core::inventory::{ByteRange, SourceRevision};
    /// use objekte_search_core::lifecycle::Index;
    /// use objekte_search_core::model::Value;
    /// use objekte_search_core::schema::Schema;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let root = std::env::temp_dir().join(format!("objekte-doc-links-{}", std::process::id()));
    /// std::fs::create_dir(&root)?;
    /// std::fs::write(root.join("source.txt"), b"alpha beta")?;
    /// let revision = SourceRevision::from_metadata(&std::fs::metadata(root.join("source.txt"))?)?;
    /// let mut builder = Index::builder();
    /// let collection = builder.collection("entries", Schema::default())?;
    /// let registration = builder.register_records(collection, "my.linked", 1)?;
    /// let catalog = builder.register_file_catalog(FileCatalog::new(&root)?)?;
    /// builder.enable_file_links(registration, catalog)?;
    /// let index = builder.build()?;
    /// let session = index.ingestion();
    /// let writer = session.writer(registration)?;
    /// let mut batch = writer.batch();
    /// let file = batch.declare_file("source.txt", revision)?;
    /// batch.insert_with_file("a", Value::String("alpha".into()), file,
    ///     Some(ByteRange::checked(0, 5, revision.size)?))?;
    /// batch.insert_with_file("b", Value::String("beta".into()), file,
    ///     Some(ByteRange::checked(6, 10, revision.size)?))?;
    /// writer.submit(batch).unwrap().wait_published()?;
    /// let view = index.view()?;
    /// let catalog = view.file_catalog().unwrap();
    /// let link = catalog.file_for_record("entries", "a")?.unwrap();
    /// assert_eq!(catalog.records_for_file(link.file_id)?.len(), 2);
    /// session.finish()?;
    /// std::fs::remove_dir_all(root)?;
    /// # Ok(()) }
    /// ```
    pub fn enable_file_links(
        &mut self,
        registration: RecordRegistration,
        catalog: FileCatalogRegistration,
    ) -> Result<(), IndexError> {
        if catalog.builder_id != self.builder_id || self.catalog_root.is_none() {
            return Err(IndexError::ForeignInstance);
        }
        if registration.builder_id != self.builder_id {
            return Err(IndexError::ForeignInstance);
        }
        let source = self
            .registrations
            .get_mut(registration.ordinal as usize)
            .filter(|source| source.handle == registration)
            .ok_or(IndexError::ForeignInstance)?;
        source.file_links = true;
        Ok(())
    }

    /// Publish a usable empty index with no file discovery or source reads.
    pub fn build(self) -> Result<Index, IndexError> {
        if self.file_sources.iter().any(Option::is_some) && self.catalog_root.is_none() {
            return Err(IndexError::Build(
                "file sources require a file catalog".into(),
            ));
        }
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
        let mut repository = crate::construction::repository_state::initialize_empty_programmatic(
            self.collections,
            self.options,
            &identities,
            self.catalog_root,
            self.file_sources,
            &self.sources,
        )
        .map_err(IndexError::Build)?;
        repository.record_registrations = std::sync::Arc::new(self.registrations);
        Index::from_repository(repository)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_batch_publishes_and_old_view_remains_empty() {
        let mut builder = Index::builder();
        let mut schema = Schema::default();
        schema
            .register_field(crate::schema::FieldDefinition {
                path: vec!["year".into()],
                kind: crate::model::LogicalType::Number,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
        let entries = builder.collection("entries", schema).unwrap();
        let registration = builder
            .register_records(entries, "test.entries", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let before = index.view().unwrap();
        let id = crate::model::RecordId::intrinsic("entry-47").unwrap();
        let value = crate::model::Value::Object(std::collections::BTreeMap::from([
            (
                "year".into(),
                crate::model::Value::Number(crate::model::Number::integer(1808)),
            ),
            ("note".into(), crate::model::Value::String("Leipzig".into())),
        ]));
        let declaration = &before.repository().collections[0].definition;
        let options = before.repository().options();
        let (headers, bodies, contribution) =
            crate::construction::batch::build_retained_contribution(
                vec![(id.clone(), value.clone(), Vec::new())],
                declaration,
                &options,
            )
            .unwrap();
        let draft = crate::construction::segment_admission::CollectionSegment::from_retained(
            crate::index::segment::SegmentId::new(2),
            entries,
            "test.entries".into(),
            1,
            registration.ordinal,
            headers,
            bodies,
            contribution,
        )
        .unwrap();
        let segment = draft
            .into_indexed(
                declaration,
                options.bigram_alphabet,
                options.inference,
                before.repository().collections.len(),
                &before.repository().workers,
            )
            .unwrap();
        let published = index
            .publish_record_batch(
                before.revision(),
                entries,
                registration.ordinal,
                &[crate::lifecycle::publication::RecordMutation {
                    id,
                    kind: crate::lifecycle::publication::RecordMutationKind::Insert,
                }],
                Some(segment),
            )
            .unwrap();
        let after = index.view().unwrap();
        assert_eq!(after.revision(), published);
        assert!(
            before
                .get_record(
                    "entries",
                    "entry-47",
                    crate::query::options::ReturnMode::Locator
                )
                .unwrap()
                .is_none()
        );
        let hit = after
            .get_record(
                "entries",
                "entry-47",
                crate::query::options::ReturnMode::Logical,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            hit.payload,
            Some(crate::query_results::Payload::Logical(value))
        );
        assert!(hit.file.is_none());
        let query = crate::query::Query {
            collections: vec!["entries".into()],
            fts: None,
            filter: None,
            options: crate::query::QueryOptions {
                sort: Vec::new(),
                limit: 10,
                offset: 0,
                count: true,
                return_mode: crate::query::options::ReturnMode::Locator,
                evidence: false,
            },
        };
        assert_eq!(after.search(&query).unwrap().count, Some(1));
        let mut exact = query.clone();
        exact.filter = Some(crate::query::parse_filter("year = 1808").unwrap());
        exact.options.limit = 0;
        assert_eq!(after.search(&exact).unwrap().count, Some(1));
        let mut residual = query;
        residual.filter =
            Some(crate::query::parse_filter("year = 1808 && note ~ 'Leipzig'").unwrap());
        let result = after.search(&residual).unwrap();
        assert_eq!(result.count, Some(1));
        assert_eq!(result.hits[0].record_id, "entry-47");
    }

    #[test]
    fn builder_publishes_empty_fileless_index_with_frozen_registrations() {
        let mut builder = Index::builder();
        let entries = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(entries, "application.entries", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let view = index.view().unwrap();
        assert_eq!(view.repository().record_registrations.len(), 1);
        assert_eq!(
            view.repository().record_registrations[0].handle,
            registration
        );
        assert_eq!(
            view.collection_declarations().next().unwrap().name,
            "entries"
        );
        assert_eq!(view.index_observation().files, 0);
        assert_eq!(view.index_observation().records, 0);
        assert!(view.file_catalog().is_none());
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

    #[test]
    fn explicitly_installed_empty_catalog_is_visible_and_scoped() {
        let mut without = Index::builder();
        let without_entries = without.collection("entries", Schema::default()).unwrap();
        without
            .register_records(without_entries, "application.entries", 1)
            .unwrap();
        let without_identity = without
            .build()
            .unwrap()
            .view()
            .unwrap()
            .configuration_identity();
        let mut builder = Index::builder();
        let entries = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(entries, "application.entries", 1)
            .unwrap();
        let catalog = builder
            .register_file_catalog(FileCatalog::new(std::env::temp_dir()).unwrap())
            .unwrap();
        builder.enable_file_links(registration, catalog).unwrap();
        let view = builder.build().unwrap().view().unwrap();
        let files = view.file_catalog().unwrap();
        assert_ne!(view.configuration_identity(), without_identity);
        assert_eq!(view.index_observation().files, 0);
        assert_eq!(
            files.records_for_file(crate::model::FileId(0)),
            Err(crate::files::LookupError::FileNotFound)
        );
    }
}
