use super::*;
use objekte_search_core::inventory::{DiagnosticRef, FileStatus, RecordClass};

#[test]
fn raw_fallback_export_keeps_status_and_explicit_unavailable_state() {
    let mut hit = SearchHit {
        collection: "data".into(),
        record_id: "~fallback:test".into(),
        generation: 1,
        file: Some(objekte_search_core::files::RecordFileInfo {
            file_id: objekte_search_core::model::FileId(0),
            path: "bad.json".into(),
            revision: objekte_search_core::inventory::SourceRevision {
                size: 5,
                modified_ns: 1,
                object_id: None,
            },
            created_ns: None,
            status: FileStatus::Broken,
            range: Some(objekte_search_core::inventory::ByteRange { start: 0, end: 5 }),
        }),
        matched_fields: vec!["@record.raw_text".into()],
        record_class: RecordClass::RawFallback,
        diagnostic_ref: DiagnosticRef(1),
        payload: Some(Payload::Bytes(b"{bad}".to_vec())),
    };
    let bytes = hit_value(&hit).unwrap();
    assert_eq!(bytes["file_status"], 1);
    assert_eq!(bytes["record_class"], 1);
    assert_eq!(bytes["record"], "{bad}");
    assert_eq!(bytes["diagnostic_ref"], 1);
    hit.payload = Some(Payload::LogicalUnavailable {
        diagnostic_ref: DiagnosticRef(1),
    });
    let logical = hit_value(&hit).unwrap();
    assert!(logical["record"].is_null());
    assert_eq!(logical["logical_unavailable"], true);
}

#[test]
fn unlinked_hit_keeps_record_identity_and_emits_null_file_metadata() {
    let mut hit = SearchHit {
        collection: "entries".into(),
        record_id: "entry-47".into(),
        generation: 2,
        file: None,
        matched_fields: Vec::new(),
        record_class: RecordClass::Logical,
        diagnostic_ref: DiagnosticRef(0),
        payload: Some(Payload::Logical(objekte_search_core::model::Value::Null)),
    };
    let exported = hit_value(&hit).unwrap();
    assert_eq!(exported["collection"], "entries");
    assert_eq!(exported["id"], "entry-47");
    for key in [
        "file",
        "byte_start",
        "byte_end",
        "file_status",
        "file_status_label",
    ] {
        assert!(exported[key].is_null(), "{key}");
    }
    hit.file = Some(objekte_search_core::files::RecordFileInfo {
        file_id: objekte_search_core::model::FileId(9),
        path: "data/entries.json".into(),
        revision: objekte_search_core::inventory::SourceRevision {
            size: 10,
            modified_ns: 1,
            object_id: None,
        },
        created_ns: None,
        status: FileStatus::Valid,
        range: None,
    });
    let linked = hit_value(&hit).unwrap();
    assert_eq!(linked["file"], "data/entries.json");
    assert!(linked["byte_start"].is_null());
    assert!(linked["byte_end"].is_null());
}
