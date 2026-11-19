use crate::inventory::{ByteRange, SourceRevision};
use crate::model::{FileId, RecordId, RecordSlot};
use crate::schema::{CustomBinding, FieldDefinition, Schema, Transform};
use prelude::*;

mod comparison;
mod filter;
mod folding;
mod fts;
mod prelude;
mod schema;

fn fixture_schema() -> Schema {
    let mut schema = Schema::default();
    for (path, kind, multi) in [
        ("name", LogicalType::String, false),
        (
            "values",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
        ("people.name", LogicalType::String, true),
        (
            "people.aliases",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: path.split('.').map(str::to_owned).collect(),
                kind,
                required: false,
                nullable: true,
                indexed: true,
                multi,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    schema
        .register_custom(
            vec!["name".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    schema
}

fn with_record<F: FnOnce(RecordContext<'_>)>(value: Value, f: F) {
    let record = RecordEntry {
        slot: RecordSlot(0),
        id: RecordId("x".into()),
        file: FileId(0),
        range: ByteRange { start: 0, end: 4 },
        revision: SourceRevision {
            modified_ns: 0,
            size: 4,
        },
        class: crate::inventory::RecordClass::Logical,
        diagnostic_ref: crate::inventory::DiagnosticRef(0),
    };
    let file = FileEntry {
        id: FileId(0),
        relative_path: "x.json".into(),
        revision: record.revision,
        created_ns: None,
        collections: vec![],
        status: crate::inventory::FileStatus::Valid,
        diagnostics: vec![],
        omitted_diagnostics: 0,
    };
    f(RecordContext {
        value: &value,
        source_bytes: None,
        record: &record,
        file: &file,
        collection_name: "test",
        repository_root: Path::new("/tmp"),
    });
}
