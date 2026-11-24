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
            },
            created_ns: None,
            status: FileStatus::Broken,
            range: Some(objekte_search_core::inventory::ByteRange { start: 0, end: 5 }),
        }),
        file_id: 0,
        file_path: "bad.json".into(),
        file_size: 5,
        file_created_ns: None,
        byte_start: 0,
        byte_end: 5,
        modified_ns: 1,
        matched_fields: vec!["@record.raw_text".into()],
        file_status: FileStatus::Broken,
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
