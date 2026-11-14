//! Durable store protocol: preparation, staging, atomic commit, and loading.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use uuid::Uuid;

use super::backing::{
    acquire_reader_lease, acquire_writer_lease, try_acquire_exclusive_reader_lease,
    Backing, MappedObject,
};
use super::error::{StorageError, StorageResult};
use super::format::{ObjectHeader, ObjectKind, ObjectView, FORMAT_VERSION};
use super::manifest::{
    hex, manifest_checksum_matches, replace_current, sha256, sync_directory, write_synced,
    CollectionSummary, CurrentPointer, GenerationManifest, ObjectRef, PathEncoding, RunRef,
    SourceRegistrationIdentity, StoreMarker, MAX_FRONTEND_METADATA_BYTES, MAX_MANIFEST_BYTES,
};

pub const STORE_FILE: &str = "STORE";
pub const CURRENT_FILE: &str = "CURRENT";
pub const OBJECTS_DIR: &str = "objects";
pub const GENERATIONS_DIR: &str = "generations";
pub const STAGING_DIR: &str = "staging";
pub const WRITER_LOCK: &str = "writer.lock";
pub const READERS_LOCK: &str = "readers.lock";
pub const MANIFEST_FILE: &str = "manifest.json";

/// One object to persist.
#[derive(Clone)]
pub struct ObjectInput {
    pub kind: ObjectKind,
    pub bytes: Arc<[u8]>,
}

/// One run to persist.
#[derive(Clone)]
pub struct RunInput {
    pub collection: u32,
    pub ordinal: u64,
    pub level: u32,
    pub slot_count: u32,
    pub active_count: u64,
    pub object: ObjectInput,
}

/// One complete save request.
pub struct SavePlan {
    pub publication_id: Uuid,
    pub lineage_id: Uuid,
    pub source_root: PathBuf,
    pub model_version: u32,
    pub configuration_identity: [u8; 32],
    pub source_registrations: Vec<SourceRegistrationIdentity>,
    pub collation_version: String,
    pub bigram_representation_version: String,
    pub file_id_high_water: u32,
    pub catalogue: ObjectInput,
    pub state: Option<ObjectInput>,
    pub runs: Vec<RunInput>,
    pub collections: Vec<CollectionSummary>,
    pub frontend_metadata: Option<serde_json::Value>,
}

/// Save options controlled by the caller.
#[derive(Clone, Debug, Default)]
pub struct SaveOptions {
    pub frontend_metadata: Option<serde_json::Value>,
}

/// Save phases reported through progress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SavePhase {
    Preparing,
    WritingObjects,
    WritingCatalogue,
    WritingManifest,
    Committing,
    CleaningUp,
    Complete,
    Cancelled,
    Failed,
}

/// Observable progress for one save.
#[derive(Clone, Debug)]
pub struct SaveProgress {
    pub phase: SavePhase,
    pub pinned_publication: Uuid,
    pub objects_total: u64,
    pub objects_completed: u64,
    pub bytes_written: u64,
    pub bytes_reused: u64,
    pub elapsed: Duration,
}

/// Cleanup outcome; failures never invalidate a committed save.
#[derive(Clone, Debug, Default)]
pub struct CleanupReport {
    pub readers_present: bool,
    pub generations_removed: u64,
    pub objects_removed: u64,
    pub staging_removed: u64,
    pub errors: Vec<String>,
}

/// Final save report.
#[derive(Clone, Debug)]
pub struct SaveReport {
    pub directory: PathBuf,
    pub store_id: Uuid,
    pub generation_id: Uuid,
    pub publication_id: Uuid,
    pub objects_written: u64,
    pub objects_reused: u64,
    pub bytes_written: u64,
    pub bytes_reused: u64,
    pub catalogue_bytes: u64,
    pub duration: Duration,
    pub unchanged: bool,
    pub newer_publication_exists: bool,
    pub cleanup: CleanupReport,
}

/// Fault-injection and observation hooks used by durability tests.
pub trait SaveHooks {
    fn before_object_write(&mut self, _sha256: &str) -> StorageResult<()> {
        Ok(())
    }
    fn after_object_staged(&mut self, _sha256: &str) -> StorageResult<()> {
        Ok(())
    }
    fn before_manifest_write(&mut self) -> StorageResult<()> {
        Ok(())
    }
    fn before_current_replace(&mut self) -> StorageResult<()> {
        Ok(())
    }
    fn after_current_replace(&mut self) -> StorageResult<()> {
        Ok(())
    }
    fn during_cleanup(&mut self) -> StorageResult<()> {
        Ok(())
    }
}

/// Default hook set.
pub struct NoHooks;

impl SaveHooks for NoHooks {}

/// One opened run with its mapped object.
pub struct OpenedRun {
    pub reference: RunRef,
    pub object: Arc<MappedObject>,
}

impl std::fmt::Debug for OpenedRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedRun")
            .field("collection", &self.reference.collection)
            .field("ordinal", &self.reference.ordinal)
            .field("level", &self.reference.level)
            .field("slot_count", &self.reference.slot_count)
            .finish_non_exhaustive()
    }
}

/// One opened committed store generation.
pub struct OpenedStore {
    pub directory: PathBuf,
    pub marker: StoreMarker,
    pub manifest: GenerationManifest,
    pub current: CurrentPointer,
    pub catalogue: Arc<MappedObject>,
    pub state: Option<Arc<MappedObject>>,
    pub runs: Vec<OpenedRun>,
    lease: Arc<super::backing::ReaderLease>,
}

impl std::fmt::Debug for OpenedStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedStore")
            .field("directory", &self.directory)
            .field("store_id", &self.marker.store_id)
            .field("generation", &self.current.generation)
            .field("runs", &self.runs.len())
            .finish_non_exhaustive()
    }
}

/// Lightweight manifest inspection result.
#[derive(Clone, Debug)]
pub struct StoredIndexInfo {
    pub directory: PathBuf,
    pub marker: StoreMarker,
    pub manifest: GenerationManifest,
    pub current: CurrentPointer,
    pub manifest_bytes: u64,
}

impl OpenedStore {
    pub fn lease(&self) -> Arc<super::backing::ReaderLease> {
        Arc::clone(&self.lease)
    }

    pub fn object_backing(&self, object: &Arc<MappedObject>) -> Backing {
        Backing::mapped(Arc::clone(object))
    }

    pub fn catalogue_backing(&self) -> Backing {
        Backing::mapped(Arc::clone(&self.catalogue))
    }

    pub fn run_backing(&self, run: &OpenedRun) -> Backing {
        Backing::mapped(Arc::clone(&run.object))
    }

    /// Validate every referenced object checksum. Normal load does not call
    /// this; explicit diagnostics do.
    pub fn verify_full(&self) -> StorageResult<()> {
        ObjectView::new(&self.catalogue_backing())?.verify_full()?;
        if let Some(state) = &self.state {
            ObjectView::new(&Backing::mapped(Arc::clone(state)))?.verify_full()?;
        }
        for run in &self.runs {
            ObjectView::new(&self.run_backing(run))?.verify_full()?;
        }
        Ok(())
    }
}

/// Open and validate the committed generation. No source files are touched.
pub fn open_store(directory: &Path) -> StorageResult<OpenedStore> {
    let lease = acquire_reader_lease(&directory.join(READERS_LOCK))?;
    let marker = read_store_marker(directory)?;
    let (current, manifest_bytes) = read_current_manifest(directory, &marker)?;
    let manifest = GenerationManifest::decode(&manifest_bytes)?;
    manifest.validate(&marker)?;
    if manifest.generation_id != current.generation.to_string() {
        return Err(StorageError::InvalidManifest(
            "CURRENT generation differs from the manifest".into(),
        ));
    }
    let manifest_path = generation_manifest_path(directory, current.generation);
    let manifest_len = fs::metadata(&manifest_path)
        .map_err(|error| StorageError::io(&manifest_path, error))?
        .len();
    if manifest_len != manifest_bytes.len() as u64 {
        return Err(StorageError::InvalidManifest(
            "manifest length changed during load".into(),
        ));
    }
    let catalogue = open_object(
        directory,
        &manifest.catalogue,
        ObjectKind::Catalogue,
        Some(Arc::clone(&lease)),
    )?;
    let state = match &manifest.state {
        Some(reference) => Some(open_object(
            directory,
            reference,
            ObjectKind::State,
            Some(Arc::clone(&lease)),
        )?),
        None => None,
    };
    let mut runs = Vec::with_capacity(manifest.runs.len());
    for reference in &manifest.runs {
        let object = open_object(
            directory,
            &reference.object,
            ObjectKind::Segment,
            Some(Arc::clone(&lease)),
        )?;
        runs.push(OpenedRun {
            reference: reference.clone(),
            object,
        });
    }
    Ok(OpenedStore {
        directory: directory.to_owned(),
        marker,
        manifest,
        current,
        catalogue,
        state,
        runs,
        lease,
    })
}

/// Inspect the manifest and marker without opening source files.
pub fn inspect_store(directory: &Path) -> StorageResult<StoredIndexInfo> {
    let lease = acquire_reader_lease(&directory.join(READERS_LOCK))?;
    let marker = read_store_marker(directory)?;
    let (current, manifest_bytes) = read_current_manifest(directory, &marker)?;
    let manifest = GenerationManifest::decode(&manifest_bytes)?;
    manifest.validate(&marker)?;
    drop(lease);
    Ok(StoredIndexInfo {
        directory: directory.to_owned(),
        marker,
        manifest,
        current,
        manifest_bytes: manifest_bytes.len() as u64,
    })
}

fn read_store_marker(directory: &Path) -> StorageResult<StoreMarker> {
    let path = directory.join(STORE_FILE);
    let bytes = fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            StorageError::NotFound(path.clone())
        } else {
            StorageError::io(&path, error)
        }
    })?;
    StoreMarker::decode(&bytes)
}

fn read_current_manifest(
    directory: &Path,
    marker: &StoreMarker,
) -> StorageResult<(CurrentPointer, Vec<u8>)> {
    let current_path = directory.join(CURRENT_FILE);
    let bytes = fs::read(&current_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            StorageError::NotFound(current_path.clone())
        } else {
            StorageError::io(&current_path, error)
        }
    })?;
    let current = CurrentPointer::decode(&bytes)?;
    let manifest_path = generation_manifest_path(directory, current.generation);
    let manifest = fs::read(&manifest_path)
        .map_err(|error| StorageError::io(&manifest_path, error))?;
    if !manifest_checksum_matches(&manifest, &current) {
        return Err(StorageError::InvalidManifest(format!(
            "manifest checksum does not match CURRENT for {}",
            current.generation
        )));
    }
    if marker.format_version != FORMAT_VERSION as u32 {
        return Err(StorageError::UnsupportedFormat(format!(
            "store format version {}",
            marker.format_version
        )));
    }
    Ok((current, manifest))
}

fn generation_directory(directory: &Path, generation: Uuid) -> PathBuf {
    directory.join(GENERATIONS_DIR).join(generation.to_string())
}

fn generation_manifest_path(directory: &Path, generation: Uuid) -> PathBuf {
    generation_directory(directory, generation).join(MANIFEST_FILE)
}

fn object_path(directory: &Path, digest: &[u8; 32]) -> PathBuf {
    directory.join(OBJECTS_DIR).join(format!("{}.obx", hex(digest)))
}

fn open_object(
    directory: &Path,
    reference: &ObjectRef,
    expected: ObjectKind,
    lease: Option<Arc<super::backing::ReaderLease>>,
) -> StorageResult<Arc<MappedObject>> {
    reference.validate()?;
    let digest = reference.digest()?;
    let path = object_path(directory, &digest);
    let object = MappedObject::open(&path, lease)?;
    if object.len() != reference.length {
        return Err(StorageError::corrupt_at(
            &path,
            "object length differs from its manifest reference",
        ));
    }
    if object.header().kind != expected {
        return Err(StorageError::Incompatible(format!(
            "{} has object kind {:?}, expected {expected:?}",
            path.display(),
            object.header().kind
        )));
    }
    Ok(Arc::new(object))
}

struct PreparedObject {
    kind: ObjectKind,
    digest: [u8; 32],
    bytes: Arc<[u8]>,
}

/// Save one pinned publication to a dedicated store directory.
pub fn save(
    directory: &Path,
    plan: SavePlan,
    options: SaveOptions,
) -> StorageResult<SaveReport> {
    save_with_control(
        directory,
        plan,
        options,
        || false,
        |_| {},
        &mut NoHooks,
    )
}

/// Save with cancellation, progress, and fault-injection hooks.
pub fn save_with_control(
    directory: &Path,
    plan: SavePlan,
    options: SaveOptions,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(&SaveProgress),
    hooks: &mut dyn SaveHooks,
) -> StorageResult<SaveReport> {
    let started = Instant::now();
    let pinned_publication = plan.publication_id;
    let mut report_progress = |phase,
                               objects_total,
                               objects_completed,
                               bytes_written,
                               bytes_reused| {
        progress(&SaveProgress {
            phase,
            pinned_publication,
            objects_total,
            objects_completed,
            bytes_written,
            bytes_reused,
            elapsed: started.elapsed(),
        });
    };
    if directory.as_os_str().is_empty() {
        return Err(StorageError::InvalidArgument(
            "index store directory is empty".into(),
        ));
    }
    if same_path(directory, &plan.source_root) {
        return Err(StorageError::InvalidArgument(
            "the index store cannot be the source root".into(),
        ));
    }
    if let Some(metadata) = &options.frontend_metadata {
        let bytes = serde_json::to_vec(metadata)
            .map_err(|error| StorageError::InvalidArgument(error.to_string()))?;
        if bytes.len() > MAX_FRONTEND_METADATA_BYTES {
            return Err(StorageError::ResourceLimit(
                "frontend metadata exceeds 1 MiB".into(),
            ));
        }
    }
    report_progress(SavePhase::Preparing, 0, 0, 0, 0);
    fs::create_dir_all(directory).map_err(|error| StorageError::io(directory, error))?;
    let _writer = acquire_writer_lease(&directory.join(WRITER_LOCK))?;
    let marker = initialize_or_validate_marker(directory, plan.lineage_id)?;
    let existing = read_existing_manifest(directory, &marker)?;
    if let Some((current, existing_manifest)) = &existing {
        if existing_manifest.publication_id == plan.publication_id.to_string()
            && existing_manifest.frontend_metadata == options.frontend_metadata
        {
            report_progress(SavePhase::Complete, 0, 0, 0, 0);
            return Ok(SaveReport {
                directory: directory.to_owned(),
                store_id: marker.store_id,
                generation_id: current.generation,
                publication_id: plan.publication_id,
                objects_written: 0,
                objects_reused: 0,
                bytes_written: 0,
                bytes_reused: 0,
                catalogue_bytes: 0,
                duration: started.elapsed(),
                unchanged: true,
                newer_publication_exists: false,
                cleanup: CleanupReport::default(),
            });
        }
    }
    let catalogue_len = plan.catalogue.bytes.len() as u64;
    let mut prepared = Vec::with_capacity(plan.runs.len() + 2);
    prepared.push(prepare_object(plan.catalogue.clone())?);
    if let Some(state) = &plan.state {
        prepared.push(prepare_object(state.clone())?);
    }
    for run in &plan.runs {
        prepared.push(prepare_object(run.object.clone())?);
    }
    let mut unique: BTreeMap<[u8; 32], PreparedObject> = BTreeMap::new();
    for object in prepared {
        if let Some(existing) = unique.get(&object.digest) {
            if existing.kind != object.kind || existing.bytes.len() != object.bytes.len() {
                return Err(StorageError::InvalidArgument(
                    "one SHA-256 digest names incompatible objects".into(),
                ));
            }
        } else {
            unique.insert(object.digest, object);
        }
    }
    let objects_total = unique.len() as u64;
    let staging = directory.join(STAGING_DIR).join(Uuid::new_v4().to_string());
    fs::create_dir_all(&staging).map_err(|error| StorageError::io(&staging, error))?;
    let objects_directory = directory.join(OBJECTS_DIR);
    fs::create_dir_all(&objects_directory)
        .map_err(|error| StorageError::io(&objects_directory, error))?;
    let mut objects_written = 0u64;
    let mut objects_reused = 0u64;
    let mut bytes_written = 0u64;
    let mut bytes_reused = 0u64;
    let mut objects_completed = 0u64;
    let mut object_refs = BTreeMap::new();
    let outcome = (|| -> StorageResult<Uuid> {
        for (digest, object) in &unique {
            if cancelled() {
                return Err(StorageError::Cancelled);
            }
            hooks.before_object_write(&hex(digest))?;
            let target = object_path(directory, digest);
            if let Some(reused) = reused_object(&target, object)? {
                objects_reused += 1;
                bytes_reused += reused;
                object_refs.insert(
                    *digest,
                    ObjectRef::new(*digest, object.bytes.len() as u64),
                );
                objects_completed += 1;
                report_progress(
                    SavePhase::WritingObjects,
                    objects_total,
                    objects_completed,
                    bytes_written,
                    bytes_reused,
                );
                continue;
            }
            let staged = staging.join(format!("{}.obx", hex(digest)));
            write_object(&staged, object)?;
            hooks.after_object_staged(&hex(digest))?;
            if cancelled() {
                return Err(StorageError::Cancelled);
            }
            if let Err(error) = fs::rename(&staged, &target) {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    let _ = fs::remove_file(&staged);
                } else {
                    return Err(StorageError::io(&target, error));
                }
            }
            let metadata = fs::metadata(&target)
                .map_err(|error| StorageError::io(&target, error))?;
            if metadata.len() != object.bytes.len() as u64 {
                return Err(StorageError::corrupt_at(
                    &target,
                    "published object length differs from the staged object",
                ));
            }
            objects_written += 1;
            bytes_written += object.bytes.len() as u64;
            object_refs.insert(
                *digest,
                ObjectRef::new(*digest, object.bytes.len() as u64),
            );
            objects_completed += 1;
            report_progress(
                SavePhase::WritingObjects,
                objects_total,
                objects_completed,
                bytes_written,
                bytes_reused,
            );
        }
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        hooks.before_manifest_write()?;
        let generation = Uuid::new_v4();
        let manifest = build_manifest(&plan, &options, &marker, generation, &object_refs)?;
        let manifest_bytes = manifest.encode()?;
        if manifest_bytes.len() > MAX_MANIFEST_BYTES {
            return Err(StorageError::ResourceLimit(
                "manifest exceeds the maximum size".into(),
            ));
        }
        let manifest_hash = sha256(&manifest_bytes);
        report_progress(
            SavePhase::WritingManifest,
            objects_total,
            objects_completed,
            bytes_written,
            bytes_reused,
        );
        let generations = directory.join(GENERATIONS_DIR);
        fs::create_dir_all(&generations).map_err(|error| StorageError::io(&generations, error))?;
        let destination = generation_directory(directory, generation);
        if destination.exists() {
            return Err(StorageError::Conflict(
                "generation directory already exists".into(),
            ));
        }
        let staged_manifest = staging.join(MANIFEST_FILE);
        write_synced(&staged_manifest, &manifest_bytes)?;
        fs::create_dir_all(&destination)
            .map_err(|error| StorageError::io(&destination, error))?;
        fs::rename(
            &staged_manifest,
            destination.join(MANIFEST_FILE),
        )
        .map_err(|error| StorageError::io(&destination, error))?;
        sync_directory(&destination)?;
        sync_directory(&generations)?;
        sync_directory(&objects_directory)?;
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        hooks.before_current_replace()?;
        let pointer = CurrentPointer {
            generation,
            manifest_sha256: manifest_hash,
        };
        report_progress(
            SavePhase::Committing,
            objects_total,
            objects_completed,
            bytes_written,
            bytes_reused,
        );
        replace_current(directory, &pointer)?;
        sync_directory(directory)?;
        Ok(generation)
    })();
    let generation = match outcome {
        Ok(generation) => generation,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            report_progress(
                if matches!(error, StorageError::Cancelled) {
                    SavePhase::Cancelled
                } else {
                    SavePhase::Failed
                },
                objects_total,
                objects_completed,
                bytes_written,
                bytes_reused,
            );
            return Err(error);
        }
    };
    // The commit point has passed. Hook or cleanup failures below are
    // reported in the cleanup outcome and never turn commitment into failure.
    let mut cleanup_errors = Vec::new();
    if let Err(error) = hooks.after_current_replace() {
        cleanup_errors.push(error.to_string());
    }
    report_progress(
        SavePhase::CleaningUp,
        objects_total,
        objects_completed,
        bytes_written,
        bytes_reused,
    );
    let mut cleanup = cleanup_after_commit(directory, generation, &marker, hooks);
    cleanup.errors.extend(cleanup_errors);
    let _ = fs::remove_dir_all(&staging);
    let newer_publication_exists = current_publication(directory, &marker)
        .ok()
        .flatten()
        .is_some_and(|manifest| manifest.publication_id != plan.publication_id.to_string());
    report_progress(
        SavePhase::Complete,
        objects_total,
        objects_completed,
        bytes_written,
        bytes_reused,
    );
    Ok(SaveReport {
        directory: directory.to_owned(),
        store_id: marker.store_id,
        generation_id: generation,
        publication_id: plan.publication_id,
        objects_written,
        objects_reused,
        bytes_written,
        bytes_reused,
        catalogue_bytes: catalogue_len,
        duration: started.elapsed(),
        unchanged: false,
        newer_publication_exists,
        cleanup,
    })
}

fn prepare_object(input: ObjectInput) -> StorageResult<PreparedObject> {
    if input.bytes.len() < super::format::HEADER_LEN {
        return Err(StorageError::InvalidArgument(
            "object bytes are shorter than a valid header".into(),
        ));
    }
    let header = ObjectHeader::parse(&input.bytes[..super::format::HEADER_LEN])?;
    if header.kind != input.kind {
        return Err(StorageError::InvalidArgument(
            "object kind differs from its header".into(),
        ));
    }
    if header.total_len != input.bytes.len() as u64 {
        return Err(StorageError::InvalidArgument(
            "object length differs from its header".into(),
        ));
    }
    let digest = sha256(&input.bytes);
    Ok(PreparedObject {
        kind: input.kind,
        digest,
        bytes: input.bytes,
    })
}

fn reused_object(path: &Path, object: &PreparedObject) -> StorageResult<Option<u64>> {
    match fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(StorageError::io(path, error)),
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(StorageError::corrupt_at(
                    path,
                    "published object path is not a regular file",
                ));
            }
            if metadata.len() != object.bytes.len() as u64 {
                return Err(StorageError::corrupt_at(
                    path,
                    "existing object length conflicts with its content address",
                ));
            }
            Ok(Some(metadata.len()))
        }
    }
}

fn write_object(path: &Path, object: &PreparedObject) -> StorageResult<()> {
    let mut file = fs::File::create(path).map_err(|error| StorageError::io(path, error))?;
    for chunk in object.bytes.chunks(1024 * 1024) {
        file.write_all(chunk)
            .map_err(|error| StorageError::io(path, error))?;
    }
    file.sync_all().map_err(|error| StorageError::io(path, error))?;
    Ok(())
}

fn build_manifest(
    plan: &SavePlan,
    options: &SaveOptions,
    marker: &StoreMarker,
    generation: Uuid,
    objects: &BTreeMap<[u8; 32], ObjectRef>,
) -> StorageResult<GenerationManifest> {
    let catalogue_digest = sha256(&plan.catalogue.bytes);
    let _catalogue = objects
        .get(&catalogue_digest)
        .cloned()
        .ok_or_else(|| StorageError::InvalidManifest("catalogue object was not staged".into()))?;
    let mut runs = Vec::with_capacity(plan.runs.len());
    for run in &plan.runs {
        let digest = sha256(&run.object.bytes);
        let object = objects
            .get(&digest)
            .cloned()
            .ok_or_else(|| StorageError::InvalidManifest("run object was not staged".into()))?;
        runs.push(RunRef {
            collection: run.collection,
            ordinal: run.ordinal,
            level: run.level,
            object,
            slot_count: run.slot_count,
            active_count: run.active_count,
        });
    }
    Ok(GenerationManifest {
        manifest_version: super::manifest::MANIFEST_VERSION,
        format_version: FORMAT_VERSION,
        model_version: plan.model_version,
        path_encoding: marker.path_encoding.code(),
        store_id: marker.store_id.to_string(),
        lineage_id: marker.lineage_id.to_string(),
        publication_id: plan.publication_id.to_string(),
        generation_id: generation.to_string(),
        source_root: hex(&path_bytes(&plan.source_root)),
        configuration_identity: hex(&plan.configuration_identity),
        source_registrations: plan.source_registrations.clone(),
        collation_version: plan.collation_version.clone(),
        bigram_representation_version: plan.bigram_representation_version.clone(),
        file_id_high_water: plan.file_id_high_water,
        catalogue: ObjectRef::new(catalogue_digest, plan.catalogue.bytes.len() as u64),
        state: plan
            .state
            .as_ref()
            .map(|state| {
                let digest = sha256(&state.bytes);
                objects.get(&digest).cloned().ok_or_else(|| {
                    StorageError::InvalidManifest("state object was not staged".into())
                })
            })
            .transpose()?,
        runs,
        collections: plan.collections.clone(),
        frontend_metadata: options.frontend_metadata.clone(),
    })
}

fn initialize_or_validate_marker(directory: &Path, lineage_id: Uuid) -> StorageResult<StoreMarker> {
    let marker_path = directory.join(STORE_FILE);
    if marker_path.exists() {
        let marker = read_store_marker(directory)?;
        if marker.lineage_id != lineage_id {
            return Err(StorageError::Incompatible(
                "store belongs to a different index lineage".into(),
            ));
        }
        return Ok(marker);
    }
    let entries = fs::read_dir(directory)
        .map_err(|error| StorageError::io(directory, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| StorageError::io(directory, error))?;
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !matches!(
            name.as_ref(),
            OBJECTS_DIR | GENERATIONS_DIR | STAGING_DIR | WRITER_LOCK | READERS_LOCK | "CURRENT.tmp"
        ) {
            return Err(StorageError::InvalidArgument(format!(
                "store directory is not dedicated to an Objekte index: unexpected {}",
                entry.path().display()
            )));
        }
    }
    let marker = StoreMarker::new(lineage_id);
    write_synced(&marker_path, &marker.encode())?;
    fs::create_dir_all(directory.join(OBJECTS_DIR))
        .map_err(|error| StorageError::io(&directory.join(OBJECTS_DIR), error))?;
    fs::create_dir_all(directory.join(GENERATIONS_DIR))
        .map_err(|error| StorageError::io(&directory.join(GENERATIONS_DIR), error))?;
    sync_directory(directory)?;
    Ok(marker)
}

fn read_existing_manifest(
    directory: &Path,
    marker: &StoreMarker,
) -> StorageResult<Option<(CurrentPointer, GenerationManifest)>> {
    match read_current_manifest(directory, marker) {
        Ok((current, bytes)) => Ok(Some((current, GenerationManifest::decode(&bytes)?))),
        Err(StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

fn current_publication(
    directory: &Path,
    marker: &StoreMarker,
) -> StorageResult<Option<GenerationManifest>> {
    match read_existing_manifest(directory, marker) {
        Ok(Some((_, manifest))) => Ok(Some(manifest)),
        Ok(None) => Ok(None),
        Err(error) => Err(error),
    }
}

fn cleanup_after_commit(
    directory: &Path,
    current_generation: Uuid,
    marker: &StoreMarker,
    hooks: &mut dyn SaveHooks,
) -> CleanupReport {
    let mut report = CleanupReport::default();
    if let Err(error) = hooks.during_cleanup() {
        report.errors.push(error.to_string());
    }
    let readers = match try_acquire_exclusive_reader_lease(&directory.join(READERS_LOCK)) {
        Ok(Some(lease)) => Some(lease),
        Ok(None) => {
            report.readers_present = true;
            return report;
        }
        Err(error) => {
            report.errors.push(error.to_string());
            return report;
        }
    };
    let generations_directory = directory.join(GENERATIONS_DIR);
    let mut generations = Vec::new();
    match fs::read_dir(&generations_directory) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .and_then(|name| Uuid::parse_str(name).ok())
                        .is_some()
                {
                    let modified = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    generations.push((modified, path));
                }
            }
        }
        Err(error) => report.errors.push(error.to_string()),
    }
    generations.sort_by(|left, right| right.0.cmp(&left.0));
    let mut retained = BTreeSet::new();
    retained.insert(current_generation.to_string());
    let mut previous = None;
    for (_, path) in &generations {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        if name == current_generation.to_string() {
            continue;
        }
        if previous.is_none() {
            previous = Some(name);
        }
    }
    if let Some(previous) = &previous {
        retained.insert(previous.clone());
    }
    let mut retained_objects = HashSet::new();
    for generation in &retained {
        if let Ok(generation) = Uuid::parse_str(generation)
            && let Ok(bytes) = fs::read(generation_manifest_path(directory, generation))
            && let Ok(manifest) = GenerationManifest::decode(&bytes)
        {
            retained_objects.insert(manifest.catalogue.sha256);
            if let Some(state) = &manifest.state {
                retained_objects.insert(state.sha256.clone());
            }
            for run in manifest.runs {
                retained_objects.insert(run.object.sha256);
            }
        }
    }
    for (_, path) in generations {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        if retained.contains(&name) {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => report.generations_removed += 1,
            Err(error) => report.errors.push(format!("{}: {error}", path.display())),
        }
    }
    let objects_directory = directory.join(OBJECTS_DIR);
    if let Ok(entries) = fs::read_dir(&objects_directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            if !retained_objects.contains(name) {
                match fs::remove_file(&path) {
                    Ok(()) => report.objects_removed += 1,
                    Err(error) => report.errors.push(format!("{}: {error}", path.display())),
                }
            }
        }
    }
    let staging_directory = directory.join(STAGING_DIR);
    if let Ok(entries) = fs::read_dir(&staging_directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                match fs::remove_dir_all(&path) {
                    Ok(()) => report.staging_removed += 1,
                    Err(error) => report.errors.push(format!("{}: {error}", path.display())),
                }
            }
        }
    }
    let _ = marker;
    drop(readers);
    report
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_owned());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_owned());
    left == right
}

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}

/// Remove one staging directory left by a failed test or interrupted save.
pub fn remove_staging(directory: &Path, operation: Uuid) -> StorageResult<()> {
    let path = directory.join(STAGING_DIR).join(operation.to_string());
    match fs::remove_dir_all(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StorageError::io(&path, error)),
    }
}
