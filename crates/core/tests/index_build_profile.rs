//! The opt-in profile must include every compressed text partition.
#![cfg(feature = "index-timings")]

use objekte_core::{BigramEncoding, Repository, file_picker::FilePicker};
use std::fs;

#[test]
fn field_build_and_secondary_save_have_separate_measurements() {
    let root = tempfile::tempdir().unwrap();
    let schema = root.path().join(".objekte/collections/docs.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        &schema,
        r#"{"type":"object","properties":{"body":{"type":"string","x-full-text":"markdown"}},"x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    let data = root.path().join("data/docs");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("a.json"), r#"{"body":"**hello** world"}"#).unwrap();
    fs::write(data.join("b.json"), r#"{"body":"another document"}"#).unwrap();

    let repository = Repository::open(root.path()).unwrap();
    let collection = repository.model().collections.get("docs").unwrap();
    let picker = FilePicker::profile_collection(collection, BigramEncoding::Ascii).unwrap();
    let stages = picker.build_timings();
    assert_eq!(picker.len(), 2);
    assert!(stages.source_read.wall_nanos > 0);
    assert!(stages.json_parse.wall_nanos > 0);
    assert!(stages.general_values.wall_nanos > 0);
    assert!(stages.field_text.wall_nanos > 0);
    assert!(stages.main_insert.wall_nanos > 0);
    assert!(stages.rendered_insert.wall_nanos > 0);
    assert!(stages.field_insert.wall_nanos > 0);
    assert_eq!(stages.main_compress.wall_nanos, stages.compress_nanos);
    assert!(stages.rendered_compress.wall_nanos > 0);
    assert!(stages.field_compress.wall_nanos > 0);
    #[cfg(unix)]
    {
        assert!(stages.source_read.cpu_nanos > 0);
        assert!(stages.field_text.cpu_nanos > 0);
        assert!(stages.field_insert.cpu_nanos > 0);
        assert!(stages.field_compress.cpu_nanos > 0);
    }

    let sizes = picker.profile_index_sizes();
    assert_eq!(sizes.fields.len(), 1);
    assert_eq!(sizes.fields[0].path, "body");
    assert!(sizes.fields[0].normal_bytes > 0);
    assert!(sizes.fields[0].rendered_bytes > 0);
    assert_eq!(
        picker.index_bytes(),
        sizes.main_bytes
            + sizes.rendered_global_bytes
            + sizes.fields[0].normal_bytes
            + sizes.fields[0].rendered_bytes
    );

    let snapshot = root.path().join("profile.bigram");
    picker.save_bigram_snapshot(&snapshot).unwrap();
    let secondary = picker.profile_secondary_snapshot(&snapshot).unwrap();
    assert!(secondary.wall_nanos > 0);
    assert!(snapshot.with_extension("secondary").is_file());

    let mut repository = Repository::open(root.path()).unwrap();
    let warmed = repository.warm_bigram_index("docs").unwrap();
    assert!(warmed.bigram_compress_nanos > 0);
    assert!(warmed.bigram_rendered_compress_nanos > 0);
    assert!(warmed.bigram_field_compress_nanos > 0);
    let rebuilt = repository.rebuild_bigram_index("docs").unwrap();
    assert!(rebuilt.bigram_compress_nanos > 0);
    assert!(rebuilt.bigram_rendered_compress_nanos > 0);
    assert!(rebuilt.bigram_field_compress_nanos > 0);
}
