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
        files: CompactFilesRef<'a>,
        changed_slots: &'a [usize],
        changed_bigrams: &'a BTreeMap<usize, Vec<u16>>,
    }


    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let files = vec![BigramFile {
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
            files: CompactFilesRef(&files),
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
fn rejects_unknown_manifest_encoding_with_valid_checksum() {
    let temp = tempfile::tempdir().unwrap();
    let filter = crate::bigram::build_bigram_index_from_content_with_encoding(
        &[Some("Müller".as_bytes())],
        BigramEncoding::Utf8Bytes,
    );
    let files = vec![BigramFile {
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
            files: CompactFilesRef(&files),
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
