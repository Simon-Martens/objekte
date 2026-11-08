use std::path::PathBuf;

use super::*;

#[test]
fn plain_folder_and_missing_path_are_distinct() {
    let folder = tempfile::tempdir().unwrap();
    assert_eq!(
        detect_source(folder.path()).unwrap(),
        DetectedSource::PlainFolder
    );
    assert!(detect_source(&folder.path().join("missing")).is_err());
}

#[test]
fn existing_configured_repository_is_recognized_without_opening_data() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    assert_eq!(
        detect_source(&root).unwrap(),
        DetectedSource::ConfiguredRepository { collections: 7 }
    );
}

#[test]
fn malformed_configuration_does_not_fall_back_to_plain_folder() {
    let folder = tempfile::tempdir().unwrap();
    let definitions = folder.path().join(".objekte/collections");
    std::fs::create_dir_all(&definitions).unwrap();
    std::fs::write(definitions.join("broken.schema.json"), "not JSON").unwrap();
    let result = detect_source(folder.path()).unwrap();
    assert!(
        matches!(result, DetectedSource::InvalidConfiguration { message } if message.contains("broken.schema.json"))
    );
}
