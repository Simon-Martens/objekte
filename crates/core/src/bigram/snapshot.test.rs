use super::*;

fn write_test_snapshot(path: &Path, magic: &[u8; 8], manifest: &[u8], filter: &[u8]) {
    let mut digest = Sha256::new();
    digest.update(manifest);
    digest.update(filter);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(magic);
    bytes.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(filter.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&digest.finalize());
    bytes.extend_from_slice(manifest);
    bytes.extend_from_slice(filter);
    fs::write(path, bytes).unwrap();
}

#[test]
fn reads_legacy_json_manifest_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let filter = crate::bigram::build_bigram_index_from_content(&[Some(b"alpha")]);
    let manifest = serde_json::to_vec(&Manifest {
        bigram_encoding: BigramEncoding::Ascii,
        source_root: temp.path().to_owned(),
        recursive: false,
        git_commit: None,
        files: vec![BigramFile {
            id: "a".into(),
            filename: "a.json".into(),
            size_bytes: 5,
            modified_unix_nanos: "1".into(),
            identity: String::new(),
        }],
        changed_slots: Vec::new(),
        changed_bigrams: BTreeMap::new(),
    })
    .unwrap();
    let encoded_filter = filter.to_bytes().unwrap();
    let path = temp.path().join("legacy.bigram");
    write_test_snapshot(&path, MAGIC_JSON, &manifest, &encoded_filter);

    let restored = BigramSnapshot::load(&path).unwrap();
    assert_eq!(restored.files[0].filename, "a.json");
    assert_eq!(restored.candidate_files(b"alpha").len(), 1);
}

#[test]
fn reads_legacy_binary_manifest_as_ascii() {
    /// Original compact binary manifest without an encoding field.
    #[derive(Serialize)]
    struct Legacy<'a> {
        source_root: &'a PathBuf,
        recursive: bool,
        git_commit: &'a Option<String>,
        files: CompactFilesRef<'a, [BigramFile]>,
        changed_slots: &'a [usize],
        changed_bigrams: &'a BTreeMap<usize, Vec<u16>>,
    }


    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let files = [BigramFile {
        id: "a".into(),
        filename: "a.json".into(),
        size_bytes: 5,
        modified_unix_nanos: "1".into(),
        identity: String::new(),
    }];
    let filter = crate::bigram::build_bigram_index_from_content(&[Some(b"alpha")]);
    let manifest = bincode::DefaultOptions::new()
        .serialize(&Legacy {
            source_root: &root,
            recursive: false,
            git_commit: &None,
            files: CompactFilesRef(&files[..]),
            changed_slots: &[],
            changed_bigrams: &BTreeMap::new(),
        })
        .unwrap();
    let path = temp.path().join("legacy-v2.bigram");
    write_test_snapshot(&path, MAGIC_BINARY, &manifest, &filter.to_bytes().unwrap());
    let restored = BigramSnapshot::load(&path).unwrap();
    assert_eq!(restored.filter.encoding(), BigramEncoding::Ascii);
    assert_eq!(restored.candidate_files(b"alpha").len(), 1);

    let utf8 = crate::bigram::build_bigram_index_from_content_with_encoding(
        &[Some("alpha".as_bytes())],
        BigramEncoding::Utf8Bytes,
    );
    write_test_snapshot(&path, MAGIC_BINARY, &manifest, &utf8.to_bytes().unwrap());
    assert!(BigramSnapshot::load(&path).is_err());
}

#[test]
fn reads_previous_encoding_and_scalar_snapshot_versions() {
    let temp = tempfile::tempdir().unwrap();
    let files = [BigramFile {
        id: "a".into(),
        filename: "a.json".into(),
        size_bytes: 5,
        modified_unix_nanos: "1".into(),
        identity: String::new(),
    }];
    let filter = crate::bigram::build_bigram_index_from_content(&[Some(b"alpha")]);
    let manifest = bincode::DefaultOptions::new()
        .serialize(&ManifestBinaryRef {
            source_root: &temp.path().to_path_buf(),
            recursive: false,
            git_commit: &None,
            files: CompactFilesRef(&files[..]),
            changed_slots: &[],
            changed_bigrams: &BTreeMap::new(),
            bigram_encoding: "ascii",
        })
        .unwrap();
    for (magic, scalar) in [(MAGIC_ENCODING, false), (MAGIC_SCALAR_POSTINGS, true)] {
        let path = temp.path().join(format!("legacy-{scalar}.bigram"));
        write_test_snapshot(&path, magic, &manifest, &filter.to_bytes().unwrap());
        let loaded = BigramSnapshot::load(&path).unwrap();
        assert_eq!(loaded.files, files);
        assert_eq!(loaded.uses_scalar_postings(), scalar);
        let compact = BigramSnapshot::load_scoped_compact(&path, temp.path(), false).unwrap();
        assert_eq!(compact.id_bytes, b"a");
    }
}

#[test]
fn rejects_unknown_manifest_encoding_with_valid_checksum() {
    let temp = tempfile::tempdir().unwrap();
    let filter = crate::bigram::build_bigram_index_from_content_with_encoding(
        &[Some("Müller".as_bytes())],
        BigramEncoding::Utf8Bytes,
    );
    let files = [BigramFile {
        id: "a".into(),
        filename: "a.json".into(),
        size_bytes: 7,
        modified_unix_nanos: "1".into(),
        identity: String::new(),
    }];
    let source_root = temp.path().to_path_buf();
    let manifest = bincode::DefaultOptions::new()
        .serialize(&ManifestBinaryRef {
            source_root: &source_root,
            recursive: false,
            git_commit: &None,
            files: CompactFilesRef(&files[..]),
            changed_slots: &[],
            changed_bigrams: &BTreeMap::new(),
            bigram_encoding: "utf8_bytes",
        })
        .unwrap();
    let mut corrupt = manifest.clone();
    let at = corrupt
        .windows(b"utf8_bytes".len())
        .position(|window| window == b"utf8_bytes")
        .unwrap();
    corrupt[at..at + 10].copy_from_slice(b"utf8_other");
    let path = temp.path().join("unknown.bigram");
    write_test_snapshot(&path, MAGIC_ENCODING, &corrupt, &filter.to_bytes().unwrap());
    assert!(BigramSnapshot::load(&path).is_err());
}

#[test]
fn compact_snapshot_round_trips_both_encodings_and_overlay() {
    let temp = tempfile::tempdir().unwrap();
    for encoding in [BigramEncoding::Ascii, BigramEncoding::Utf8Bytes] {
        let filter = crate::bigram::build_bigram_index_from_content_with_encoding(
            &[Some("Müller Straße".as_bytes())],
            encoding,
        );
        let path = temp.path().join(format!("{}.bigram", encoding.as_str()));
        let snapshot = BigramSnapshot {
            source_root: temp.path().to_owned(),
            recursive: false,
            git_commit: None,
            files: vec![BigramFile {
                id: "a".into(),
                filename: "a.json".into(),
                size_bytes: 14,
                modified_unix_nanos: "-1234567890123456789".into(),
                identity: "11:22:-3:4".into(),
            }],
            changed_slots: vec![0],
            changed_bigrams: BTreeMap::from([(0, vec![12, 13])]),
            filter,
        };
        snapshot.save(&path).unwrap();
        assert_eq!(&fs::read(&path).unwrap()[..8], MAGIC_COMPACT);
        let restored = BigramSnapshot::load(&path).unwrap();
        assert_eq!(restored.files, snapshot.files);
        assert_eq!(restored.changed_slots, snapshot.changed_slots);
        assert_eq!(restored.changed_bigrams, snapshot.changed_bigrams);
        let compact = BigramSnapshot::load_scoped_compact(&path, temp.path(), false).unwrap();
        assert_eq!(compact.id_bytes, b"a");
        assert_eq!(
            compact.entries[0].as_ref().unwrap().stamp.modified,
            "-1234567890123456789"
        );
        let other = tempfile::tempdir().unwrap();
        assert!(BigramSnapshot::load_scoped_compact(&path, other.path(), false).is_err());
    }
}

#[test]
fn compact_manifest_rejects_bad_offsets_ids_and_encoding() {
    let temp = tempfile::tempdir().unwrap();
    let filter = crate::bigram::build_bigram_index_from_content(&[Some(b"alpha")]);
    let encoded_filter = filter.to_bytes().unwrap();
    let path = temp.path().join("corrupt.bigram");
    let mut manifest = compact::Manifest {
        source_root: temp.path().to_owned(),
        recursive: false,
        git_commit: None,
        id_bytes: b"a".to_vec(),
        entries: vec![compact::FileEntry {
            id_start: 0,
            id_len: 1,
            size_bytes: 5,
            modified_unix_nanos: 1,
            identity: compact::FileIdentity::Empty,
        }],
        changed_slots: Vec::new(),
        changed_bigrams: BTreeMap::new(),
        bigram_encoding: "ascii".into(),
        scalar_postings: false,
    };
    let write = |manifest: &compact::Manifest| {
        let bytes = bincode::DefaultOptions::new().serialize(manifest).unwrap();
        write_test_snapshot(&path, MAGIC_COMPACT, &bytes, &encoded_filter);
    };
    write(&manifest);
    assert!(BigramSnapshot::load(&path).is_ok());
    manifest.entries[0].id_start = 1;
    write(&manifest);
    assert!(BigramSnapshot::load(&path).is_err());
    manifest.entries[0].id_start = 0;
    manifest.id_bytes = vec![0xff];
    write(&manifest);
    assert!(BigramSnapshot::load(&path).is_err());
    manifest.id_bytes = b"a".to_vec();
    manifest.bigram_encoding = "utf8_bytes".into();
    write(&manifest);
    assert!(BigramSnapshot::load(&path).is_err());

    let two = crate::bigram::build_bigram_index_from_content(&[Some(b"alpha"), Some(b"beta")]);
    manifest.bigram_encoding = "ascii".into();
    manifest.id_bytes = b"aa".to_vec();
    manifest.entries.push(compact::FileEntry {
        id_start: 1,
        id_len: 1,
        size_bytes: 4,
        modified_unix_nanos: 2,
        identity: compact::FileIdentity::Empty,
    });
    let bytes = bincode::DefaultOptions::new().serialize(&manifest).unwrap();
    write_test_snapshot(&path, MAGIC_COMPACT, &bytes, &two.to_bytes().unwrap());
    assert!(BigramSnapshot::load(&path).is_err());
}

#[test]
fn public_unordered_snapshot_keeps_legacy_round_trip_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let snapshot = BigramSnapshot {
        source_root: temp.path().to_owned(),
        recursive: false,
        git_commit: None,
        files: vec![
            BigramFile {
                id: "b".into(),
                filename: "b.json".into(),
                size_bytes: 1,
                modified_unix_nanos: "1".into(),
                identity: String::new(),
            },
            BigramFile {
                id: "a".into(),
                filename: "a.json".into(),
                size_bytes: 1,
                modified_unix_nanos: "2".into(),
                identity: String::new(),
            },
        ],
        changed_slots: Vec::new(),
        changed_bigrams: BTreeMap::new(),
        filter: crate::bigram::build_bigram_index_from_content(&[Some(b"b"), Some(b"a")]),
    };
    let path = temp.path().join("unordered.bigram");
    snapshot.save(&path).unwrap();
    assert_eq!(&fs::read(&path).unwrap()[..8], MAGIC_ENCODING);
    assert_eq!(BigramSnapshot::load(&path).unwrap().files, snapshot.files);
    assert!(BigramSnapshot::load_scoped_compact(&path, temp.path(), false).is_err());
}
