use super::*;
use crate::bigram::BigramIndex;
use crate::model::{FieldPath, LogicalType};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::{CustomBinding, FieldDefinition, Schema, Transform};
use crate::repository::{Repository, RepositoryOptions, SnapshotMode};
use crate::source::{SourceFormat, TextSegment};
use crate::workload::ReturnMode;

/// Diagnostic prototype for a direct batch-to-shard fresh build. This only
/// measures the dominant `contents` collection and does not publish an index.
#[test]
#[ignore = "manual real-fixture direct-build feasibility measurement"]
fn measure_direct_shard_build_from_real_worker_batches() {
    let root = std::path::PathBuf::from(
        std::env::var_os("OBJEKTE_DIRECT_SHARD_FIXTURE")
            .expect("set OBJEKTE_DIRECT_SHARD_FIXTURE to a Musenalm root"),
    );
    let configuration = crate::repository_configuration::load_repository_configuration(&root)
        .unwrap();
    let definitions = configuration.definitions;
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        ..Default::default()
    };
    let baseline_started = std::time::Instant::now();
    let repository = Repository::open_with_options(&root, definitions, options).unwrap();
    eprintln!("direct shard prototype static_build={:?}", baseline_started.elapsed());
    let collection = repository
        .collections()
        .iter()
        .find(|collection| collection.definition.name == "contents")
        .unwrap();
    let id = collection.generation.collection;
    let definition = Arc::new(collection.definition.clone());
    let source = collection.source.clone();
    let source_files = Arc::new(
        repository
            .files()
            .files
            .iter()
            .map(SourceFileInput::from_entry)
            .collect::<Vec<_>>(),
    );
    let jobs = file_batches(&collection.generation.files, repository.files())
        .into_iter()
        .map(|files| RepositoryFileJob {
            collection_index: id.0 as usize,
            collection: id,
            definition: Arc::clone(&definition),
            source: source.clone(),
            files: files.to_vec(),
        })
        .collect::<Vec<_>>();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(repository.startup().build_workers)
        .build()
        .unwrap();
    let new_admission = || {
        SegmentAdmission::with_limits(
            id,
            &source,
            mutation_segment::SegmentLimits {
                files: 2_048,
                records: 2_048,
                source_bytes: 64 * 1024 * 1024,
            },
        )
    };
    let mut admission = new_admission();
    let mut chunks = Vec::new();
    let started = std::time::Instant::now();
    let stats = process_repository_file_batches(
        &pool,
        Arc::new(root.clone()),
        &jobs,
        source_files,
        options.bigram_alphabet,
        options.inference,
        |_, batch| {
            if admission.record_count() > 0
                && (admission.record_count() + batch.records.len() > 2_048
                    || admission.file_count() + batch.files.len() > 2_048)
            {
                chunks.push(std::mem::replace(&mut admission, new_admission()).seal());
            }
            admission.admit_batch(batch)
        },
    )
    .unwrap();
    if admission.file_count() > 0 {
        chunks.push(admission.seal());
    }
    let extracted = started.elapsed();
    let started = std::time::Instant::now();
    let shards = chunks
        .into_iter()
        .map(|chunk| {
            chunk.into_indexed(
                &definition,
                options.bigram_alphabet,
                options.inference,
                repository.collections().len(),
                &pool,
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let finalized = started.elapsed();
    let records = shards.iter().map(|shard| shard.records.len()).sum::<usize>();
    assert_eq!(records, collection.generation.records.len());
    assert!(shards.iter().all(|shard| shard.records.len() <= 2_048));
    eprintln!(
        "direct shard prototype files={} records={} shards={} extract_admit={extracted:?} finalize={finalized:?} jobs={} peak_active={}",
        collection.generation.files.len(), records, shards.len(), jobs.len(),
        stats.peak_active_jobs,
    );
}

#[test]
fn fresh_build_reads_capture_whole_file_digest_without_another_read() {
    let root = std::env::temp_dir().join(format!(
        "objekte-build-digests-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("shared.jsonl");
    let bytes = b"{\"name\":\"one\"}\n{broken}\n{\"name\":\"three\"}\n";
    std::fs::write(&path, bytes).unwrap();
    let input = SourceFileInput {
        id: FileId(0),
        relative_path: "shared.jsonl".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let definition = CollectionDefinition::folder("shared", ".", SourceFormat::JsonLines);
    let source = crate::source::SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let expected = Some((input.id, Sha256::digest(bytes).into()));
    let batch = process_batch(
        &root,
        &definition,
        CollectionId(0),
        &source,
        &[input.id],
        &[input.clone()],
        BigramAlphabet::Ascii,
        InferenceConfig::default(),
        1,
    )
    .unwrap();
    assert_eq!(batch.records.len(), 3);
    assert_eq!(batch.file_fingerprints(), &[expected.unwrap()]);
    let profiled = process_batch_profiled(
        &root,
        &definition,
        CollectionId(0),
        &source,
        &[input.id],
        &[input.clone()],
        BigramAlphabet::Ascii,
        InferenceConfig::default(),
        1,
        true,
    )
    .unwrap();
    assert_eq!(profiled.records.len(), batch.records.len());
    assert_eq!(profiled.file_fingerprints(), batch.file_fingerprints());
    assert_eq!(profiled.record_parses, batch.record_parses);
    assert_eq!(profiled.profile.sampled_files, 1);
    assert_eq!(profiled.profile.sampled_file_bytes, bytes.len() as u64);
    assert!(profiled.profile.sampled_read_calls > 0);
    assert_eq!(profiled.profile.sampled_source_records, 3);
    assert_eq!(profiled.profile.sampled_component_records, 1);

    std::fs::remove_file(&path).unwrap();
    let missing = process_batch(
        &root,
        &definition,
        CollectionId(0),
        &source,
        &[input.id],
        &[input.clone()],
        BigramAlphabet::Ascii,
        InferenceConfig::default(),
        1,
    )
    .unwrap();
    assert!(missing.file_fingerprints().is_empty());
    std::fs::write(&path, bytes).unwrap();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut admitted = 0;
    let bounded = process_framed_file(
        &pool,
        Arc::new(root.clone()),
        Arc::new(definition),
        CollectionId(0),
        source,
        input.id,
        Arc::new(vec![input]),
        BigramAlphabet::Ascii,
        InferenceConfig::default(),
        |batch| {
            admitted += batch.records.len();
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(admitted, 3);
    assert_eq!(bounded.file_fingerprint, expected);

    let json_bytes = b"{\"name\":\"single\"}";
    std::fs::write(root.join("single.json"), json_bytes).unwrap();
    let json_input = SourceFileInput {
        id: FileId(0),
        relative_path: "single.json".into(),
        revision: SourceRevision::from_metadata(
            &std::fs::metadata(root.join("single.json")).unwrap(),
        )
        .unwrap(),
        created_ns: None,
    };
    let json_source = crate::source::SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let generic = process_framed_file(
        &pool,
        Arc::new(root.clone()),
        Arc::new(CollectionDefinition::folder("single", ".", SourceFormat::Json)),
        CollectionId(0),
        json_source,
        json_input.id,
        Arc::new(vec![json_input]),
        BigramAlphabet::Ascii,
        InferenceConfig::default(),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(
        generic.file_fingerprint,
        Some((FileId(0), Sha256::digest(json_bytes).into()))
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn assert_same_preparation(file: &PreparedFileBuild, supplied: &PreparedFileBuild) {
    if file.fingerprint.is_some() {
        assert_eq!(file.fingerprint, supplied.fingerprint);
    }
    assert_eq!(file.source.id, supplied.source.id);
    assert_eq!(file.source.revision, supplied.source.revision);
    assert_eq!(file.collections.len(), supplied.collections.len());
    for (expected, actual) in file.collections.iter().zip(&supplied.collections) {
        assert_eq!(expected.collection, actual.collection);
        assert_eq!(expected.source_id, actual.source_id);
        assert_eq!(expected.source_version, actual.source_version);
        assert_eq!(expected.contributions.len(), actual.contributions.len());
        for (expected, actual) in expected.contributions.iter().zip(&actual.contributions) {
            assert_eq!(expected.record_parses, actual.record_parses);
            assert_eq!(expected.failed_parses, actual.failed_parses);
            assert_eq!(expected.files[0].first_range, actual.files[0].first_range);
            assert_eq!(expected.files[0].entry.status, actual.files[0].entry.status);
            assert_eq!(
                expected.files[0].entry.diagnostics,
                actual.files[0].entry.diagnostics
            );
            assert_eq!(expected.records.len(), actual.records.len());
            for (expected, actual) in expected.records.iter().zip(&actual.records) {
                assert_eq!(expected.id, actual.id);
                assert_eq!(expected.file, actual.file);
                assert_eq!(expected.range, actual.range);
                assert_eq!(expected.revision, actual.revision);
                assert_eq!(expected.class, actual.class);
            }
        }
    }
}

#[test]
fn supplied_bytes_match_shared_jsonl_and_never_touch_source_path() {
    let root = std::env::temp_dir().join(format!(
        "objekte-supplied-jsonl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("shared.jsonl");
    let definitions = [
        CollectionDefinition::folder("first", ".", SourceFormat::JsonLines),
        CollectionDefinition::folder("second", ".", SourceFormat::JsonLines),
    ];
    let source = crate::source::SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let bindings = definitions
        .iter()
        .enumerate()
        .map(|(index, definition)| FilePreparationBinding {
            collection: CollectionId(index as u32),
            definition,
            source: &source,
        })
        .collect::<Vec<_>>();
    let inference = InferenceConfig {
        mode: crate::inference::InferenceMode::Disabled,
        policy: Default::default(),
    };
    for bytes in [
        b"".as_slice(),
        b"{\"name\":\"one\"}\n{broken}\n{\"name\":\"three\"}\n".as_slice(),
    ] {
        std::fs::write(&path, bytes).unwrap();
        let input = SourceFileInput {
            id: FileId(91),
            relative_path: "shared.jsonl".into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: Some(123),
        };
        let expected = prepare_file_build(
            &root,
            input.clone(),
            &bindings,
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        std::fs::remove_file(&path).unwrap();
        let supplied = prepare_file_build_from_bytes(
            &root,
            input,
            bytes,
            &bindings,
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        assert_same_preparation(&expected, &supplied);
        assert!(!path.exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn supplied_bytes_match_registered_shifted_ranges_and_reject_wrong_size() {
    use crate::source::{
        DiagnosticCategory, ReaderDiagnostic, ReaderFailure, SourceReader, SourceRegistry,
    };
    use sha2::Digest;


    struct ShiftedReader;


    impl SourceReader for ShiftedReader {
        fn name(&self) -> &'static str {
            "shifted"
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            if bytes != b"header|one|two|" {
                return Err("unsupported document".into());
            }
            Ok(vec![
                ByteRange::checked(7, 10, bytes.len() as u64)?,
                ByteRange::checked(11, 14, bytes.len() as u64)?,
            ])
        }

        fn frame_outcome(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, ReaderFailure> {
            if bytes == b"badframe" {
                return Err(ReaderFailure::Malformed(ReaderDiagnostic::new(
                    DiagnosticCategory::Framing,
                    "shifted.frame",
                    "invalid shifted frame",
                )));
            }
            self.frame(bytes).map_err(ReaderFailure::Fatal)
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            if bytes != b"one" && bytes != b"two" {
                return Err("unexpected record".into());
            }
            Ok(ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            })
        }
    }


    let root = std::env::temp_dir().join(format!(
        "objekte-supplied-shifted-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("shifted.shift");
    let bytes = b"header|one|two|";
    std::fs::write(&path, bytes).unwrap();
    let mut sources = SourceRegistry::builtin();
    sources
        .register("shifted", 1, "shift", std::sync::Arc::new(ShiftedReader))
        .unwrap();
    let definition =
        CollectionDefinition::folder("shifted", ".", SourceFormat::registered("shifted"));
    let source = sources
        .resolve(&SourceFormat::registered("shifted"))
        .unwrap();
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let input = SourceFileInput {
        id: FileId(301),
        relative_path: "shifted.shift".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let inference = InferenceConfig {
        mode: crate::inference::InferenceMode::Disabled,
        policy: Default::default(),
    };
    let expected = prepare_file_build(
        &root,
        input.clone(),
        &[binding],
        BigramAlphabet::Ascii,
        inference,
        1,
    )
    .unwrap();
    std::fs::remove_file(&path).unwrap();
    let supplied = prepare_file_build_from_bytes(
        &root,
        input.clone(),
        bytes,
        &[binding],
        BigramAlphabet::Ascii,
        inference,
        1,
    )
    .unwrap();
    assert_same_preparation(&expected, &supplied);
    assert_eq!(
        supplied.collections[0].contributions[0].records[0]
            .range
            .start,
        7
    );
    assert_eq!(
        supplied.collections[0].contributions[0].records[1]
            .range
            .start,
        11
    );
    assert!(!path.exists());
    let mut wrong = input.clone();
    wrong.revision.size += 1;
    assert_eq!(
        prepare_file_build_from_bytes(
            &root,
            wrong,
            bytes,
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .err(),
        Some(BytesPreparationError::RevisionSizeMismatch {
            declared: bytes.len() as u64 + 1,
            actual: bytes.len() as u64,
        })
    );
    let unsupported = b"unsupported";
    let unsupported_input = SourceFileInput {
        revision: SourceRevision {
            modified_ns: 77,
            size: unsupported.len() as u64,
        },
        ..input.clone()
    };
    assert_eq!(
        prepare_file_build_from_bytes(
            &root,
            unsupported_input,
            unsupported,
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .err(),
        Some(BytesPreparationError::UnsupportedReader {
            source_id: "shifted".into(),
            reason: "unsupported document".into(),
        })
    );
    let malformed = b"badframe";
    std::fs::write(&path, malformed).unwrap();
    let malformed_input = SourceFileInput {
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        ..input
    };
    let expected = prepare_file_build(
        &root,
        malformed_input.clone(),
        &[binding],
        BigramAlphabet::Ascii,
        inference,
        1,
    )
    .unwrap();
    std::fs::remove_file(&path).unwrap();
    let supplied = prepare_file_build_from_bytes(
        &root,
        malformed_input,
        malformed,
        &[binding],
        BigramAlphabet::Ascii,
        inference,
        1,
    )
    .unwrap();
    assert_same_preparation(&expected, &supplied);
    assert_eq!(
        supplied.fingerprint,
        Some(sha2::Sha256::digest(malformed).into())
    );
    assert!(!path.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn supplied_bytes_match_json_and_broken_fallback_without_file() {
    let root = std::env::temp_dir().join(format!(
        "objekte-supplied-json-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("data.json");
    let definition = CollectionDefinition::folder("data", ".", SourceFormat::Json);
    let source = crate::source::SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: crate::inference::InferenceMode::Disabled,
        policy: Default::default(),
    };
    for bytes in [b"{\"name\":\"one\"}".as_slice(), b"{broken}".as_slice()] {
        std::fs::write(&path, bytes).unwrap();
        let input = SourceFileInput {
            id: FileId(19),
            relative_path: "data.json".into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: None,
        };
        let expected = prepare_file_build(
            &root,
            input.clone(),
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        std::fs::remove_file(&path).unwrap();
        let supplied = prepare_file_build_from_bytes(
            &root,
            input,
            bytes,
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        assert_same_preparation(&expected, &supplied);
        assert!(!path.exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn single_file_preparation_handles_jsonl_cardinality_and_shared_membership() {
    use sha2::{Digest, Sha256};

    let root = std::env::temp_dir().join(format!(
        "objekte-file-preparation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let definitions = [
        CollectionDefinition::folder("first", ".", SourceFormat::JsonLines),
        CollectionDefinition::folder("second", ".", SourceFormat::JsonLines),
    ];
    let source = crate::source::SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let bindings = [
        FilePreparationBinding {
            collection: CollectionId(0),
            definition: &definitions[0],
            source: &source,
        },
        FilePreparationBinding {
            collection: CollectionId(1),
            definition: &definitions[1],
            source: &source,
        },
    ];
    let inference = InferenceConfig {
        mode: crate::inference::InferenceMode::Disabled,
        policy: Default::default(),
    };
    for (bytes, expected) in [
        (b"".as_slice(), 0),
        (b"{\"name\":\"one\"}\n".as_slice(), 1),
        (
            b"{\"name\":\"one\"}\n{\"name\":\"two\"}\n{\"name\":\"three\"}\n".as_slice(),
            3,
        ),
    ] {
        let path = root.join("shared.jsonl");
        std::fs::write(&path, bytes).unwrap();
        let input = SourceFileInput {
            id: FileId(91),
            relative_path: "shared.jsonl".into(),
            revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            created_ns: Some(123),
        };
        let prepared =
            prepare_file_build(&root, input, &bindings, BigramAlphabet::Ascii, inference, 1)
                .unwrap();
        assert_eq!(prepared.source.id, FileId(91));
        assert_eq!(prepared.source.created_ns, Some(123));
        assert_eq!(prepared.fingerprint, Some(Sha256::digest(bytes).into()));
        assert_eq!(prepared.collections.len(), 2);
        for (index, collection) in prepared.collections.iter().enumerate() {
            assert_eq!(collection.collection, CollectionId(index as u32));
            assert_eq!(collection.source_id, "jsonl");
            assert_eq!(collection.source_version, 1);
            assert_eq!(collection.contributions.len(), 1);
            let batch = &collection.contributions[0];
            assert_eq!(batch.records.len(), expected);
            assert_eq!(batch.files[0].entry.id, FileId(91));
            assert_eq!(batch.files[0].entry.status, FileStatus::Valid);
            assert_eq!(
                batch.files[0].entry.collections,
                [CollectionId(index as u32)]
            );
            assert_eq!(batch.files[0].records, 0..expected);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn single_file_preparation_preserves_broken_fallback_and_bounded_chunks() {
    use sha2::{Digest, Sha256};

    let root = std::env::temp_dir().join(format!(
        "objekte-file-fallback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let definition = CollectionDefinition::folder("data", ".", SourceFormat::JsonLines);
    let source = crate::source::SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: crate::inference::InferenceMode::Disabled,
        policy: Default::default(),
    };
    let mut bytes = b"{broken}\n".to_vec();
    for index in 0..600 {
        bytes.extend_from_slice(format!("{{\"n\":{index}}}\n").as_bytes());
    }
    let path = root.join("data.jsonl");
    std::fs::write(&path, &bytes).unwrap();
    let input = SourceFileInput {
        id: FileId(500),
        relative_path: "data.jsonl".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let prepared = prepare_file_build(
        &root,
        input,
        &[binding],
        BigramAlphabet::Ascii,
        inference,
        1,
    )
    .unwrap();
    assert_eq!(prepared.fingerprint, Some(Sha256::digest(&bytes).into()));
    let contributions = &prepared.collections[0].contributions;
    assert!(contributions.len() >= 3);
    assert_eq!(
        contributions
            .iter()
            .map(|batch| batch.records.len())
            .sum::<usize>(),
        601
    );
    assert_eq!(
        contributions
            .iter()
            .map(|batch| batch.failed_parses)
            .sum::<u64>(),
        1
    );
    assert_eq!(contributions[0].files[0].entry.status, FileStatus::Broken);
    assert!(contributions[0].files[0].first_range);
    assert!(
        contributions
            .iter()
            .skip(1)
            .all(|batch| !batch.files[0].first_range)
    );
    assert!(contributions.iter().all(|batch| batch.records.len() <= 256));
    let last = contributions.last().unwrap().records.last().unwrap();
    assert_eq!(last.range.end, bytes.len() as u64 - 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn single_file_preparation_rejects_stale_source_and_hashes_generic_bytes() {
    use sha2::{Digest, Sha256};
    let root = std::env::temp_dir().join(format!(
        "objekte-file-stale-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("data.json");
    std::fs::write(&path, b"{broken}").unwrap();
    let definition = CollectionDefinition::folder("data", ".", SourceFormat::Json);
    let source = crate::source::SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: crate::inference::InferenceMode::Disabled,
        policy: Default::default(),
    };
    let input = SourceFileInput {
        id: FileId(33),
        relative_path: "data.json".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let prepared = prepare_file_build(
        &root,
        input.clone(),
        &[binding],
        BigramAlphabet::Ascii,
        inference,
        1,
    )
    .unwrap();
    assert_eq!(
        prepared.fingerprint,
        Some(Sha256::digest(b"{broken}").into())
    );
    assert_eq!(
        prepared.collections[0].contributions[0].files[0]
            .entry
            .status,
        FileStatus::Broken
    );
    std::fs::write(&path, b"{\"changed\":true}").unwrap();
    assert!(
        prepare_file_build(
            &root,
            input,
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1
        )
        .err()
        .unwrap()
        .contains("stale source revision")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "manual small-file preparation timing"]
fn measure_single_file_preparation_cost() {
    let root = std::env::temp_dir().join(format!(
        "objekte-file-preparation-timing-{}",
        std::process::id(),
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("sample.jsonl");
    std::fs::write(&path, b"{\"name\":\"a small record\",\"n\":42}\n").unwrap();
    let input = SourceFileInput {
        id: FileId(1),
        relative_path: "sample.jsonl".into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        created_ns: None,
    };
    let definition = CollectionDefinition::folder("sample", ".", SourceFormat::JsonLines);
    let source = crate::source::SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: crate::inference::InferenceMode::Disabled,
        policy: Default::default(),
    };
    let started = Instant::now();
    for _ in 0..100 {
        let prepared = prepare_file_build(
            &root,
            input.clone(),
            &[binding],
            BigramAlphabet::Ascii,
            inference,
            1,
        )
        .unwrap();
        assert_eq!(prepared.collections[0].contributions[0].records.len(), 1);
    }
    eprintln!(
        "single-file JSONL preparation: {:.1} µs/call (100 calls, debug/release as invoked)",
        started.elapsed().as_secs_f64() * 10_000.0
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_worker_policy_uses_detected_capacity_and_rejects_oversubscription() {
    let nz = |value| NonZeroUsize::new(value).unwrap();
    assert_eq!(resolve_build_workers(None, Some(nz(1))), Ok(1));
    assert_eq!(resolve_build_workers(None, Some(nz(8))), Ok(8));
    assert_eq!(resolve_build_workers(None, None), Ok(1));
    assert_eq!(resolve_build_workers(Some(nz(1)), None), Ok(1));
    assert_eq!(resolve_build_workers(Some(nz(2)), Some(nz(8))), Ok(2));
    assert!(resolve_build_workers(Some(nz(9)), Some(nz(8))).is_err());
    assert!(resolve_build_workers(Some(nz(2)), None).is_err());
    assert!(NonZeroUsize::new(0).is_none());
}

#[test]
fn worker_allowance_is_bounded_and_keeps_one_oversized_job_live() {
    const MIB: u64 = 1024 * 1024;
    assert_eq!(normal_batch_allowance(1), 52 * MIB);
    assert_eq!(normal_batch_allowance(2), 52 * MIB);
    assert_eq!(normal_batch_allowance(4), 64 * MIB);
    assert_eq!(normal_batch_allowance(32), 512 * MIB);
    assert_eq!(normal_batch_allowance(usize::MAX), 512 * MIB);
    assert!(fits_allowance(0, 600 * MIB, 52 * MIB));
    assert!(!fits_allowance(8 * MIB, 600 * MIB, 52 * MIB));
    assert!(fits_allowance(44 * MIB, 8 * MIB, 52 * MIB));
    assert!(!fits_allowance(45 * MIB, 8 * MIB, 52 * MIB));
}

#[test]
fn file_workers_overlap_while_admission_keeps_file_order() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::source::{SourceReader, SourceRegistry};


    struct DelayedReader {
        active: AtomicUsize,
        peak: AtomicUsize,
        first_started: AtomicBool,
        second_started: AtomicBool,
        fail_first: bool,
        first_failed: AtomicBool,
        parsed: AtomicUsize,
    }


    impl SourceReader for DelayedReader {
        fn name(&self) -> &'static str {
            "delayed"
        }

        fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
            Ok(vec![ByteRange::checked(
                0,
                file.len() as u64,
                file.len() as u64,
            )?])
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            self.parsed.fetch_add(1, Ordering::SeqCst);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            if bytes == b"first" {
                self.first_started.store(true, Ordering::SeqCst);
                let started = std::time::Instant::now();
                while !self.second_started.load(Ordering::SeqCst) {
                    if started.elapsed().as_secs() > 5 {
                        return Err("second batch did not overlap first".into());
                    }
                    std::thread::yield_now();
                }
                if self.fail_first {
                    self.first_failed.store(true, Ordering::SeqCst);
                    self.active.fetch_sub(1, Ordering::SeqCst);
                    return Err("intentional parser failure".into());
                }
            } else {
                self.second_started.store(true, Ordering::SeqCst);
                let started = std::time::Instant::now();
                while !self.first_started.load(Ordering::SeqCst) {
                    if started.elapsed().as_secs() > 5 {
                        return Err("first batch did not overlap second".into());
                    }
                    std::thread::yield_now();
                }
                if self.fail_first {
                    let started = std::time::Instant::now();
                    while !self.first_failed.load(Ordering::SeqCst) {
                        if started.elapsed().as_secs() > 5 {
                            return Err("first batch did not report failure".into());
                        }
                        std::thread::yield_now();
                    }
                }
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            })
        }
    }


    let root = std::env::temp_dir().join(format!(
        "objekte-build-overlap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("0.delayed"), b"first").unwrap();
    std::fs::write(root.join("1.delayed"), b"second").unwrap();
    let mut inventory = FileInventory::new(root.clone());
    let ids = ["0.delayed", "1.delayed"].map(|name| {
        let metadata = std::fs::metadata(root.join(name)).unwrap();
        inventory
            .add(
                name.into(),
                SourceRevision::from_metadata(&metadata).unwrap(),
                None,
                CollectionId(0),
            )
            .unwrap()
    });
    let reader = std::sync::Arc::new(DelayedReader {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        first_started: AtomicBool::new(false),
        second_started: AtomicBool::new(false),
        fail_first: false,
        first_failed: AtomicBool::new(false),
        parsed: AtomicUsize::new(0),
    });
    let mut sources = SourceRegistry::builtin();
    sources
        .register("delayed", 1, "delayed", reader.clone())
        .unwrap();
    let definition = CollectionDefinition {
        name: "delayed".into(),
        source_path: ".".into(),
        format: SourceFormat::registered("delayed"),
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema: Default::default(),
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let jobs = [&ids[..1], &ids[1..]];
    let mut admitted = Vec::new();
    let stats = process_file_batches(
        &pool,
        std::sync::Arc::new(root.clone()),
        std::sync::Arc::new(definition.clone()),
        CollectionId(0),
        sources
            .resolve(&SourceFormat::registered("delayed"))
            .unwrap(),
        &jobs,
        std::sync::Arc::new(
            inventory
                .files
                .iter()
                .map(SourceFileInput::from_entry)
                .collect(),
        ),
        BigramAlphabet::Ascii,
        |batch| {
            admitted.push(batch.records[0].file);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(admitted, ids);
    assert_eq!(stats.weight_limit_bytes, 52 * 1024 * 1024);
    assert_eq!(stats.peak_active_jobs, 2);
    assert!(stats.peak_ready_batches >= 1);
    assert!(stats.job_execution_ns > 0);
    assert_eq!(reader.peak.load(Ordering::SeqCst), 2);
    assert_eq!(reader.active.load(Ordering::SeqCst), 0);
    let failing = std::sync::Arc::new(DelayedReader {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        first_started: AtomicBool::new(false),
        second_started: AtomicBool::new(false),
        fail_first: true,
        first_failed: AtomicBool::new(false),
        parsed: AtomicUsize::new(0),
    });
    let mut sources = SourceRegistry::builtin();
    sources
        .register("delayed", 1, "delayed", failing.clone())
        .unwrap();
    let error = process_file_batches(
        &pool,
        std::sync::Arc::new(root.clone()),
        std::sync::Arc::new(definition),
        CollectionId(0),
        sources
            .resolve(&SourceFormat::registered("delayed"))
            .unwrap(),
        &jobs,
        std::sync::Arc::new(
            inventory
                .files
                .iter()
                .map(SourceFileInput::from_entry)
                .collect(),
        ),
        BigramAlphabet::Ascii,
        |_| panic!("fatal batch must not be admitted"),
    )
    .unwrap_err();
    assert!(error.contains("intentional parser failure"), "{error}");
    assert_eq!(failing.parsed.load(Ordering::SeqCst), 2);
    assert_eq!(failing.active.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_pool_runs_file_batches_from_distinct_collections_together() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::source::{SourceReader, SourceRegistry};

    if std::thread::available_parallelism().map_or(1, NonZeroUsize::get) < 2 {
        return;
    }


    struct CrossReader {
        active: AtomicUsize,
        peak: AtomicUsize,
        second_started: AtomicBool,
    }


    impl SourceReader for CrossReader {
        fn name(&self) -> &'static str {
            "cross_collection"
        }

        fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
            Ok(vec![ByteRange::checked(
                0,
                file.len() as u64,
                file.len() as u64,
            )?])
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            let started = std::time::Instant::now();
            if bytes == b"first" {
                while !self.second_started.load(Ordering::SeqCst) {
                    if started.elapsed().as_secs() > 5 {
                        return Err("second collection did not start".into());
                    }
                    std::thread::yield_now();
                }
            } else {
                self.second_started.store(true, Ordering::SeqCst);
                while self.peak.load(Ordering::SeqCst) < 2 {
                    if started.elapsed().as_secs() > 5 {
                        return Err("first collection did not overlap".into());
                    }
                    std::thread::yield_now();
                }
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            })
        }
    }


    let root = std::env::temp_dir().join(format!(
        "objekte-build-cross-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for name in ["a", "b"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(root.join("a/0.cross"), b"first").unwrap();
    std::fs::write(root.join("b/0.cross"), b"second").unwrap();
    let reader = std::sync::Arc::new(CrossReader {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        second_started: AtomicBool::new(false),
    });
    let mut sources = SourceRegistry::builtin();
    sources
        .register("cross", 1, "cross", reader.clone())
        .unwrap();
    let definitions = ["a", "b"]
        .into_iter()
        .map(|name| CollectionDefinition {
            name: name.into(),
            source_path: name.into(),
            format: SourceFormat::registered("cross"),
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema: Default::default(),
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        })
        .collect();
    let repo = Repository::open_with_sources(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            build_workers: Some(NonZeroUsize::new(2).unwrap()),
            ..Default::default()
        },
        sources,
    )
    .unwrap();
    assert_eq!(reader.peak.load(Ordering::SeqCst), 2);
    assert_eq!(reader.active.load(Ordering::SeqCst), 0);
    assert_eq!(repo.collections()[0].generation.records.len(), 1);
    assert_eq!(repo.collections()[1].generation.records.len(), 1);
    assert_eq!(repo.collections()[0].generation.files, [FileId(0)]);
    assert_eq!(repo.collections()[1].generation.files, [FileId(1)]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_large_framed_file_uses_parallel_ranges_with_identical_indexes() {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::source::{SourceReader, SourceRegistry};


    struct TwoRanges {
        wait: bool,
        active: AtomicUsize,
        peak: AtomicUsize,
        second_started: AtomicBool,
    }


    impl SourceReader for TwoRanges {
        fn name(&self) -> &'static str {
            "two_ranges"
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            let split = bytes.iter().position(|byte| *byte == b'\n').unwrap();
            let size = bytes.len() as u64;
            Ok(vec![
                ByteRange::checked(0, split as u64, size)?,
                ByteRange::checked(split as u64 + 1, size, size)?,
            ])
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            let title = match bytes[0] {
                b'A' => {
                    if self.wait {
                        let started = std::time::Instant::now();
                        while !self.second_started.load(Ordering::SeqCst) {
                            if started.elapsed().as_secs() > 5 {
                                return Err("second range was not scheduled".into());
                            }
                            std::thread::yield_now();
                        }
                    }
                    "Alpha"
                }
                b'B' => {
                    self.second_started.store(true, Ordering::SeqCst);
                    if self.wait {
                        let started = std::time::Instant::now();
                        while self.peak.load(Ordering::SeqCst) < 2 {
                            if started.elapsed().as_secs() > 5 {
                                return Err("first range was not scheduled".into());
                            }
                            std::thread::yield_now();
                        }
                    }
                    "Beta"
                }
                _ => return Err("unexpected record".into()),
            };
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ExtractedRecord {
                value: Value::Object(BTreeMap::from([(
                    "title".into(),
                    Value::String(title.into()),
                )])),
                text: vec![TextSegment {
                    path: vec!["title".into()],
                    text: title.into(),
                }],
            })
        }
    }


    let root = std::env::temp_dir().join(format!(
        "objekte-build-framed-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut source_bytes = vec![b'a'; 4 * 1024 * 1024 + 1];
    source_bytes[0] = b'A';
    source_bytes.push(b'\n');
    source_bytes.extend(std::iter::repeat_n(b'b', 4 * 1024 * 1024 + 1));
    source_bytes[4 * 1024 * 1024 + 2] = b'B';
    std::fs::write(root.join("data/large.two"), &source_bytes).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::registered("two"),
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![FieldPath::parse("title").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let parallelism = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let build = |workers, wait| {
        let reader = std::sync::Arc::new(TwoRanges {
            wait,
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            second_started: AtomicBool::new(false),
        });
        let mut sources = SourceRegistry::builtin();
        sources.register("two", 1, "two", reader.clone()).unwrap();
        let repo = Repository::open_with_sources(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                build_workers: Some(NonZeroUsize::new(workers).unwrap()),
                ..Default::default()
            },
            sources,
        )
        .unwrap();
        (repo, reader)
    };
    let (one, serial_reader) = build(1, false);
    let (parallel, parallel_reader) = build(parallelism.min(2), parallelism >= 2);
    assert_eq!(one.collections()[0].generation.records.len(), 2);
    assert_eq!(one.build_parses(), 2);
    assert_eq!(parallel.build_parses(), 2);
    assert_eq!(serial_reader.peak.load(Ordering::SeqCst), 1);
    if parallelism >= 2 {
        assert_eq!(parallel_reader.peak.load(Ordering::SeqCst), 2);
    }
    assert_eq!(
        serde_json::to_value(&one.collections()[0].generation).unwrap(),
        serde_json::to_value(&parallel.collections()[0].generation).unwrap()
    );
    assert_eq!(
        one.collections()[0].raw.normal,
        parallel.collections()[0].raw.normal
    );
    assert_eq!(
        one.collections()[0].raw.skip,
        parallel.collections()[0].raw.skip
    );
    let path = &vec!["title".to_owned()];
    let one_posting = &one.collections()[0].postings.fields[path];
    let parallel_posting = &parallel.collections()[0].postings.fields[path];
    assert_eq!(one_posting.values, parallel_posting.values);
    assert_eq!(one_posting.presence, parallel_posting.presence);
    let sort_path = FieldPath::parse("title").unwrap();
    let one_sort = &one.collections()[0].sorting.columns[&sort_path];
    let parallel_sort = &parallel.collections()[0].sorting.columns[&sort_path];
    assert_eq!(one_sort.keys, parallel_sort.keys);
    assert_eq!(one_sort.ascending, parallel_sort.ascending);
    assert_eq!(one_sort.descending, parallel_sort.descending);
    assert_eq!(one.collections()[0].generation.files.len(), 1);
    assert_eq!(
        one.collections()[0]
            .generation
            .file_records
            .records_for(FileId(0))
            .unwrap()
            .len(),
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn oversized_jsonl_preserves_malformed_siblings_and_fallback_grams() {
    use crate::source::{
        JsonLinesReader, LogicalTextCoverage, ReaderFailure, ScanProjection, SourceReader,
        SourceRegistry, TextDerivation,
    };


    // Keep a reader on the pre-existing whole-buffer route for an independent
    // build-path comparison against the built-in bounded JSONL route.
    struct WholeJsonLines;


    impl SourceReader for WholeJsonLines {
        fn name(&self) -> &'static str {
            "jsonl"
        }

        fn logical_text_coverage(&self) -> LogicalTextCoverage {
            LogicalTextCoverage::AllStringFields
        }

        fn text_derivation(&self) -> TextDerivation {
            TextDerivation::LogicalStringsAfterConversion
        }

        fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
            JsonLinesReader.frame(bytes)
        }

        fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
            JsonLinesReader.parse(bytes)
        }

        fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
            JsonLinesReader.parse_outcome(bytes)
        }

        fn parse_for_build_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
            JsonLinesReader.parse_for_build_outcome(bytes)
        }

        fn parse_for_query(
            &self,
            bytes: &[u8],
            projection: &ScanProjection,
        ) -> Result<ExtractedRecord, String> {
            JsonLinesReader.parse_for_query(bytes, projection)
        }
    }


    let root = std::env::temp_dir().join(format!(
        "objekte-build-oversized-jsonl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut bytes = b"{\"title\":\"".to_vec();
    bytes.extend(std::iter::repeat_n(b'a', 8 * 1024 * 1024));
    bytes.extend_from_slice(b"\"}\n{\"title\":\"healthy\"}\n{\"title\":\n{\"title\":\"");
    bytes.push(0xff);
    bytes.extend_from_slice(b"\"}\n");
    std::fs::write(root.join("data/large.jsonl"), bytes).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![
            FieldPath::parse("title").unwrap(),
            FieldPath::parse("@file.status").unwrap(),
        ],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let available = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let build = |workers| {
        Repository::open_with_options(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                build_workers: Some(NonZeroUsize::new(workers).unwrap()),
                ..Default::default()
            },
        )
        .unwrap()
    };
    let one = build(1);
    let parallel = build(available.min(2));
    let mut whole_sources = SourceRegistry::default();
    whole_sources
        .register("jsonl", 1, "jsonl", std::sync::Arc::new(WholeJsonLines))
        .unwrap();
    let whole = Repository::open_with_sources(
        &root,
        vec![definition.clone()],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            build_workers: NonZeroUsize::new(1),
            ..Default::default()
        },
        whole_sources,
    )
    .unwrap();
    assert_eq!(one.build_parses(), 2);
    assert_eq!(one.build_failed_parses(), 2);
    for repo in [&one, &parallel] {
        assert_eq!(repo.collections()[0].generation.records.len(), 4);
        assert_eq!(repo.files().files[0].status, FileStatus::Broken);
        assert_eq!(repo.files().files[0].diagnostics.len(), 3);
        assert_eq!(
            repo.collections()[0]
                .generation
                .file_records
                .records_for(FileId(0))
                .unwrap()
                .len(),
            4
        );
        assert_eq!(repo.collections()[0].logical_records.len(), 2);
        for record in repo.collections()[0].generation.records.iter().skip(2) {
            assert_eq!(record.class, RecordClass::RawFallback);
            assert_eq!(
                repo.files()
                    .diagnostic(record.file, record.diagnostic_ref)
                    .unwrap()
                    .record,
                Some(record.slot)
            );
        }
    }
    assert_eq!(
        serde_json::to_value(&one.collections()[0].generation).unwrap(),
        serde_json::to_value(&parallel.collections()[0].generation).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&one.collections()[0].generation).unwrap(),
        serde_json::to_value(&whole.collections()[0].generation).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&one.files().files[0].diagnostics).unwrap(),
        serde_json::to_value(&parallel.files().files[0].diagnostics).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&one.files().files[0].diagnostics).unwrap(),
        serde_json::to_value(&whole.files().files[0].diagnostics).unwrap()
    );
    assert_eq!(
        one.collections()[0].raw.normal,
        parallel.collections()[0].raw.normal
    );
    assert_eq!(
        one.collections()[0].raw.skip,
        parallel.collections()[0].raw.skip
    );
    assert_eq!(
        one.collections()[0].fallback_text.normal,
        parallel.collections()[0].fallback_text.normal
    );
    assert_eq!(
        one.collections()[0].fallback_text.skip,
        parallel.collections()[0].fallback_text.skip
    );
    assert_eq!(
        one.collections()[0].raw.normal,
        whole.collections()[0].raw.normal
    );
    assert_eq!(
        one.collections()[0].raw.skip,
        whole.collections()[0].raw.skip
    );
    assert_eq!(
        one.collections()[0].fallback_text.normal,
        whole.collections()[0].fallback_text.normal
    );
    assert_eq!(
        one.collections()[0].fallback_text.skip,
        whole.collections()[0].fallback_text.skip
    );
    assert_eq!(
        one.collections()[0].postings.fields[&vec!["title".into()]].values,
        whole.collections()[0].postings.fields[&vec!["title".into()]].values
    );
    for filter in ["title = 'healthy'", "@record.class = 1"] {
        let query = Query {
            collections: vec!["data".into()],
            filter: Some(parse_filter(filter).unwrap()),
            fts: None,
            options: QueryOptions {
                sort: vec![],
                limit: 10,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        let a = one.search(&query).unwrap();
        let b = parallel.search(&query).unwrap();
        assert_eq!((a.count, a.hits), (b.count, b.hits), "{filter}");
    }
    one.save_default_snapshot_for_test();
    let restored = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Required,
            build_workers: NonZeroUsize::new(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(restored.startup().restored);
    assert_eq!(
        serde_json::to_value(&one.collections()[0].generation).unwrap(),
        serde_json::to_value(&restored.collections()[0].generation).unwrap()
    );
    assert_eq!(
        one.collections()[0].raw.normal,
        restored.collections()[0].raw.normal
    );
    assert_eq!(
        one.collections()[0].fallback_text.normal,
        restored.collections()[0].fallback_text.normal
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn large_jsonl_many_chunks_keep_order_and_bounded_source_reservations() {
    let root = std::env::temp_dir().join(format!(
        "objekte-build-many-jsonl-chunks-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut bytes = Vec::new();
    for _ in 0..12 {
        bytes.extend(std::iter::repeat_n(b' ', 1024 * 1024));
        bytes.push(b'\n');
    }
    let first_record_start = bytes.len() as u64;
    for index in 0..300 {
        bytes.extend_from_slice(format!("{{\"title\":\"item-{index:03}\"}}\n").as_bytes());
    }
    let source_size = bytes.len() as u64;
    std::fs::write(root.join("data/large.jsonl"), bytes).unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema: Schema::default(),
        sort_fields: Vec::new(),
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            build_workers: NonZeroUsize::new(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(repo.build_parses(), 300);
    assert_eq!(repo.build_failed_parses(), 0);
    assert_eq!(repo.files().files[0].status, FileStatus::Valid);
    let records = &repo.collections()[0].generation.records;
    assert_eq!(records.len(), 300);
    assert_eq!(records[0].range.start, first_record_start);
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].range.end < pair[1].range.start)
    );
    assert!(
        repo.startup().peak_build_admitted_bytes < source_size / 2,
        "bounded source reservations should not scale with the full file"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_jsonl_read_failure_before_or_after_admission_has_distinct_outcomes() {
    use std::io::{BufReader, Read};

    use crate::inference::{InferenceMode, InferencePolicy};
    use crate::source::{BoundedJsonLines, SourceRegistry};


    struct FailingRead {
        bytes: std::io::Cursor<Vec<u8>>,
        limit: u64,
    }


    impl Read for FailingRead {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.bytes.position() >= self.limit {
                return Err(std::io::Error::other("injected read failure"));
            }
            let count = output
                .len()
                .min((self.limit - self.bytes.position()) as usize);
            self.bytes.read(&mut output[..count])
        }
    }


    let bytes = b"{}\n".repeat(513);
    let root = std::env::temp_dir().join(format!(
        "objekte-missing-bounded-file-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let definition = Arc::new(CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema: Schema::default(),
        sort_fields: Vec::new(),
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    });
    let source_files = Arc::new(vec![SourceFileInput {
        id: FileId(0),
        relative_path: "missing.jsonl".into(),
        revision: SourceRevision {
            modified_ns: 0,
            size: bytes.len() as u64,
        },
        created_ns: None,
    }]);
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let inference = InferenceConfig {
        mode: InferenceMode::Disabled,
        policy: InferencePolicy::default(),
    };
    let make = |limit| {
        BoundedJsonLines::from_reader(BufReader::with_capacity(
            1,
            FailingRead {
                bytes: std::io::Cursor::new(bytes.clone()),
                limit,
            },
        ))
    };
    let mut before = Vec::new();
    process_bounded_json_lines_reader(
        &pool,
        Arc::new(root.clone()),
        Arc::clone(&definition),
        CollectionId(0),
        source.clone(),
        FileId(0),
        Arc::clone(&source_files),
        BigramAlphabet::Ascii,
        inference,
        false,
        make(0),
        BoundedBuildLimits::default(),
        |batch| {
            before.push((batch.records.len(), batch.files[0].entry.status));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(before, [(0, FileStatus::Unreadable)]);

    let mut blank_lines = vec![b' '; 40 * 1024];
    blank_lines.push(b'\n');
    blank_lines.extend(std::iter::repeat_n(b' ', 40 * 1024));
    blank_lines.push(b'\n');
    let mut admitted_chunks = 0;
    let stats = process_bounded_json_lines_reader(
        &pool,
        Arc::new(root.clone()),
        Arc::clone(&definition),
        CollectionId(0),
        source.clone(),
        FileId(0),
        Arc::clone(&source_files),
        BigramAlphabet::Ascii,
        inference,
        false,
        BoundedJsonLines::from_reader(BufReader::new(std::io::Cursor::new(blank_lines))),
        BoundedBuildLimits {
            chunk_bytes: 1024,
            ranges: 256,
            admitted_bytes: 64 * 1024,
        },
        |batch| {
            assert!(batch.records.is_empty());
            admitted_chunks += 1;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(admitted_chunks, 2);
    assert_eq!(stats.peak_weight_bytes, 64 * 1024);

    let mut admitted_records = 0;
    let error = process_bounded_json_lines_reader(
        &pool,
        Arc::new(root),
        definition,
        CollectionId(0),
        source,
        FileId(0),
        source_files,
        BigramAlphabet::Ascii,
        inference,
        false,
        make(512 * 3),
        BoundedBuildLimits::default(),
        |batch| {
            admitted_records += batch.records.len();
            Ok(())
        },
    )
    .err()
    .unwrap();
    assert!(error.contains("bounded framing read"), "{error}");
    assert_eq!(admitted_records, 256);
}

#[test]
fn file_batches_bound_count_and_source_bytes_without_losing_empty_files() {
    let mut inventory = FileInventory::new("/synthetic".into());
    let mut ids = Vec::new();
    for index in 0..600 {
        ids.push(
            inventory
                .add(
                    format!("{index:04}.json").into(),
                    SourceRevision {
                        modified_ns: 1,
                        size: 0,
                    },
                    None,
                    CollectionId(0),
                )
                .unwrap(),
        );
    }
    assert_eq!(
        file_batches(&ids, &inventory)
            .iter()
            .map(|batch| batch.len())
            .collect::<Vec<_>>(),
        [512, 88]
    );
    let mut large = FileInventory::new("/synthetic".into());
    let mut ids = Vec::new();
    for (index, size) in [5, 5, 9, 1].into_iter().enumerate() {
        ids.push(
            large
                .add(
                    format!("{index}.json").into(),
                    SourceRevision {
                        modified_ns: 1,
                        size: size * 1024 * 1024,
                    },
                    None,
                    CollectionId(0),
                )
                .unwrap(),
        );
    }
    assert_eq!(
        file_batches(&ids, &large)
            .iter()
            .map(|batch| batch.len())
            .collect::<Vec<_>>(),
        [1, 1, 1, 1]
    );
}

#[test]
fn bounded_batch_admission_keeps_slots_diagnostics_and_complete_gram_maps() {
    let root = std::env::temp_dir().join(format!("objekte-build-batch-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..512 {
        std::fs::write(
            root.join(format!("data/{number:04}.jsonl")),
            if number == 511 {
                b"{\"title\":\"x\",\"noise\":[]}".as_slice()
            } else {
                b"{\"title\":\"x\"}".as_slice()
            },
        )
        .unwrap();
    }
    std::fs::write(root.join("data/0512.jsonl"), b"{\"title\":").unwrap();
    std::fs::write(
        root.join("data/0513.jsonl"),
        b"{\"title\":\"y\",\"noise\":[]}",
    )
    .unwrap();
    std::fs::write(root.join("data/0514.jsonl"), b"").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_custom(
            vec!["title".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema,
        sort_fields: vec![
            FieldPath::parse("title").unwrap(),
            FieldPath::parse("@file.status").unwrap(),
        ],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::IfValid,
            build_workers: Some(NonZeroUsize::new(1).unwrap()),
            ..Default::default()
        };
        let built =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        built.save_default_snapshot_for_test();
        let restored =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert!(restored.startup().restored);
        let available = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        let four_workers = Repository::open_with_options(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                build_workers: NonZeroUsize::new(available.min(4)),
                profile_build: true,
                ..options
            },
        )
        .unwrap();
        assert!(!four_workers.startup().restored);
        let breakdown = &four_workers
            .startup()
            .build_profile
            .as_ref()
            .unwrap()
            .admission_by_collection["data"];
        assert_eq!(breakdown.parallel_batches, 0);
        assert!(breakdown.merge_join_ns >= breakdown.parallel_group_overlap_ns);
        let parallel = (available >= 8)
            .then(|| {
                Repository::open_with_options(
                    &root,
                    vec![definition.clone()],
                    RepositoryOptions {
                        snapshot_mode: SnapshotMode::Disabled,
                        build_workers: NonZeroUsize::new(8),
                        profile_build: true,
                        ..options
                    },
                )
            })
            .transpose()
            .unwrap();
        if let Some(parallel) = parallel.as_ref() {
            let breakdown = &parallel
                .startup()
                .build_profile
                .as_ref()
                .unwrap()
                .admission_by_collection["data"];
            assert!(breakdown.parallel_batches > 0);
            assert!(breakdown.merge_join_ns >= breakdown.parallel_group_overlap_ns);
        }
        for repo in [&built, &restored, &four_workers].into_iter().chain(parallel.iter()) {
            let collection = &repo.collections()[0];
            assert_eq!(collection.generation.files.len(), 515);
            assert_eq!(collection.generation.records.len(), 514);
            assert_eq!(
                collection.empty_array_records.iter().collect::<Vec<_>>(),
                [511, 513]
            );
            assert_eq!(
                collection.generation.file_records.records_for(FileId(514)),
                Some([].as_slice())
            );
            assert_eq!(repo.files().files[512].status, FileStatus::Broken);
            let malformed = &collection.generation.records[512];
            assert_eq!(malformed.class, RecordClass::RawFallback);
            assert_eq!(
                repo.files()
                    .diagnostic(malformed.file, malformed.diagnostic_ref)
                    .unwrap()
                    .record,
                Some(malformed.slot)
            );
            for (filter, count, first_id) in [
                ("title = 'x'", 512, "0000"),
                ("title = 'y'", 1, "0513"),
                ("@record.class = 1", 1, "~fallback:"),
            ] {
                let query = Query {
                    collections: vec!["data".into()],
                    filter: Some(parse_filter(filter).unwrap()),
                    fts: None,
                    options: QueryOptions {
                        sort: vec![],
                        limit: 3,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: true,
                    },
                };
                let result = repo.search(&query).unwrap();
                assert_eq!(result.count, Some(count), "{filter}");
                assert!(result.hits[0].record_id.starts_with(first_id), "{filter}");
                assert_eq!(result.counters.files_opened, 0, "{filter}");
                assert_eq!(
                    result.hits,
                    repo.search_reference(&query).unwrap().hits,
                    "{filter}"
                );
            }
            let mut direct = BigramIndex::new(alphabet);
            for slot in 0..514 {
                let text = match slot {
                    0..=511 => Some("x"),
                    513 => Some("y"),
                    _ => None,
                };
                if let Some(text) = text {
                    direct
                        .add_record(
                            RecordSlot(slot),
                            &[TextSegment {
                                path: vec!["title".into()],
                                text: text.into(),
                            }],
                        )
                        .unwrap();
                } else {
                    direct.add_empty(RecordSlot(slot)).unwrap();
                }
            }
            assert_eq!(collection.raw.normal, direct.normal);
            assert_eq!(collection.raw.skip, direct.skip);
            assert_eq!(collection.custom_text.len(), 1);
            let custom = collection.custom_text.values().next().unwrap();
            assert_eq!(custom.normal, direct.normal);
            assert_eq!(custom.skip, direct.skip);
        }
        let a = &built.collections()[0];
        let comparison = parallel.as_ref().unwrap_or(&four_workers);
        let b = &comparison.collections()[0];
        assert_eq!(
            serde_json::to_value(&a.generation).unwrap(),
            serde_json::to_value(&b.generation).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&built.files().files).unwrap(),
            serde_json::to_value(&comparison.files().files).unwrap()
        );
        assert_eq!(a.logical_records, b.logical_records);
        assert_eq!(a.empty_array_records, b.empty_array_records);
        assert_eq!(a.raw.normal, b.raw.normal);
        assert_eq!(a.raw.skip, b.raw.skip);
        assert_eq!(a.fallback_text.normal, b.fallback_text.normal);
        assert_eq!(a.fallback_text.skip, b.fallback_text.skip);
        for (path, posting) in &a.postings.fields {
            let other = &b.postings.fields[path];
            assert_eq!(posting.covered_records, other.covered_records);
            assert_eq!(posting.presence, other.presence);
            assert_eq!(posting.nulls, other.nulls);
            assert_eq!(posting.values, other.values);
            assert_eq!(posting.lengths, other.lengths);
        }
        for (path, column) in &a.sorting.columns {
            let other = &b.sorting.columns[path];
            assert_eq!(column.keys, other.keys);
            assert_eq!(column.ascending, other.ascending);
            assert_eq!(column.descending, other.descending);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn shared_file_batches_reconcile_diagnostic_caps_and_healthy_siblings() {
    let root = std::env::temp_dir().join(format!("objekte-build-shared-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut contents = "{\"title\":\n".repeat(70);
    contents.push_str("{\"title\":\"ok\"}\n");
    std::fs::write(root.join("data/mixed.jsonl"), contents).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definitions = ["a", "b"]
        .into_iter()
        .map(|name| CollectionDefinition {
            name: name.into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema: schema.clone(),
            sort_fields: vec![FieldPath::parse("@file.status").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        })
        .collect::<Vec<_>>();
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::IfValid,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    built.save_default_snapshot_for_test();
    let restored = Repository::open_with_options(&root, definitions, options).unwrap();
    assert!(restored.startup().restored);
    for repo in [&built, &restored] {
        let file = &repo.files().files[0];
        assert_eq!(file.status, FileStatus::Broken);
        assert_eq!(file.diagnostics.len(), 64);
        assert_eq!(file.omitted_diagnostics, 76);
        for collection in repo.collections() {
            assert_eq!(collection.generation.records.len(), 71);
            assert_eq!(
                collection
                    .generation
                    .file_records
                    .records_for(FileId(0))
                    .unwrap()
                    .len(),
                71
            );
            assert_eq!(
                collection.generation.records[70].class,
                RecordClass::Logical
            );
        }
        let query = |filter| Query {
            collections: vec![],
            filter: Some(parse_filter(filter).unwrap()),
            fts: None,
            options: QueryOptions {
                sort: vec![],
                limit: 3,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        for (filter, count) in [
            ("@record.class = 1", 140),
            ("title = 'ok'", 2),
            ("@file.status = 1", 142),
        ] {
            let result = repo.search(&query(filter)).unwrap();
            assert_eq!(result.count, Some(count), "{filter}");
            assert_eq!(result.counters.files_opened, 0, "{filter}");
            assert_eq!(
                result.hits,
                repo.search_reference(&query(filter)).unwrap().hits
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_intrinsic_ids_across_batches_fail_before_publication() {
    let root = std::env::temp_dir().join(format!("objekte-build-duplicate-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..513 {
        let id = if number == 0 || number == 512 {
            "same".into()
        } else {
            format!("id-{number}")
        };
        std::fs::write(
            root.join(format!("data/{number:04}.jsonl")),
            format!("{{\"id\":\"{id}\"}}\n"),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["id".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let error = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .err()
    .unwrap();
    assert!(
        error.contains("duplicate record ID") && error.contains("0512.jsonl"),
        "{error}"
    );
    std::fs::remove_dir_all(root).unwrap();
}
