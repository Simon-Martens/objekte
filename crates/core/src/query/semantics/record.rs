pub struct RecordContext<'a> {
    pub value: &'a Value,
    pub source_bytes: Option<&'a [u8]>,
    pub record: &'a dyn crate::records::RecordIdSlot,
    pub file: Option<&'a FileEntry>,
    pub range: Option<crate::inventory::ByteRange>,
    pub collection_name: &'a str,
    pub repository_root: Option<&'a Path>,
}

pub(crate) fn metadata_value(
    field: MetadataField,
    ctx: &RecordContext<'_>,
) -> Result<Option<Value>, String> {
    metadata_value_for_record(
        field,
        ctx.record.record_id(),
        ctx.record.record_class(),
        ctx.file.map(|file| (file, ctx.range)),
        ctx.collection_name,
        ctx.repository_root,
    )
}

/// Resolve a metadata field without conflating a missing file or range with a
/// declared but unknown file creation time. The borrowed file entry avoids a
/// path allocation while scanning candidates.
pub(crate) fn metadata_value_for_record(
    field: MetadataField,
    id: &crate::model::RecordId,
    class: crate::inventory::RecordClass,
    file: Option<(&FileEntry, Option<crate::inventory::ByteRange>)>,
    collection_name: &str,
    repository_root: Option<&Path>,
) -> Result<Option<Value>, String> {
    let number = |n: u64| Value::Number(Number::integer(n as i128));
    Ok(match field {
        MetadataField::RecordId => Some(Value::Id(id.0.to_string())),
        MetadataField::RecordByteStart => {
            file.and_then(|(_, range)| range.map(|range| number(range.start)))
        }
        MetadataField::RecordByteEnd => {
            file.and_then(|(_, range)| range.map(|range| number(range.end)))
        }
        MetadataField::RecordByteLength => {
            file.and_then(|(_, range)| range.map(|range| number(range.len())))
        }
        MetadataField::FileId => file.map(|(file, _)| number(file.id.0 as u64)),
        MetadataField::FilePath => file.map(|(file, _)| {
            Value::String(crate::model::path_representation::query_path(
                &file.relative_path,
            ))
        }),
        MetadataField::FileSize => file.map(|(file, _)| number(file.revision.size)),
        MetadataField::FileCreated => file
            .map(|(file, _)| {
                file.created_ns
                    .map(timestamp_value)
                    .transpose()
                    .map(|value| value.unwrap_or(Value::Null))
            })
            .transpose()?,
        MetadataField::FileModified => file
            .map(|(file, _)| timestamp_value(file.revision.modified_ns))
            .transpose()?,
        MetadataField::FileStatus => file.map(|(file, _)| number(file.status.code() as u64)),
        MetadataField::RecordClass => Some(number(class.code() as u64)),
        MetadataField::CollectionName => Some(Value::String(collection_name.into())),
        MetadataField::RepositoryRoot => repository_root
            .map(|root| Value::String(crate::model::path_representation::query_path(root))),
    })
}

fn timestamp_value(ns: i128) -> Result<Value, String> {
    time::OffsetDateTime::from_unix_timestamp_nanos(ns)
        .map(Value::Datetime)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    use crate::inventory::{FileStatus, SourceRevision};
    use crate::model::FileId;

    #[test]
    fn absent_file_and_range_differ_from_unknown_creation_time() {
        let id = crate::model::RecordId::intrinsic("entry-47").unwrap();
        let value = |field, file| {
            metadata_value_for_record(
                field,
                &id,
                crate::inventory::RecordClass::Logical,
                file,
                "entries",
                None,
            )
            .unwrap()
        };
        assert_eq!(
            value(MetadataField::RecordId, None),
            Some(Value::Id("entry-47".into()))
        );
        assert_eq!(value(MetadataField::FilePath, None), None);
        assert_eq!(value(MetadataField::FileCreated, None), None);
        assert_eq!(value(MetadataField::RecordByteStart, None), None);
        assert_eq!(value(MetadataField::RepositoryRoot, None), None);

        let file = FileEntry {
            id: FileId(4),
            relative_path: "data/a.json".into(),
            revision: SourceRevision {
                size: 100,
                modified_ns: 0,
                object_id: None,
            },
            created_ns: None,
            collections: Vec::new(),
            status: FileStatus::Valid,
            diagnostics: Vec::new(),
            omitted_diagnostics: 0,
        };
        assert_eq!(
            value(MetadataField::FileCreated, Some((&file, None))),
            Some(Value::Null)
        );
        assert_eq!(
            value(MetadataField::RecordByteStart, Some((&file, None))),
            None
        );
        assert_eq!(
            value(MetadataField::FileSize, Some((&file, None))),
            Some(Value::Number(Number::integer(100)))
        );
    }
}

fn walk_path<'a>(value: &'a Value, parts: &[String], output: &mut impl Extend<&'a Value>) -> bool {
    if parts.is_empty() {
        output.extend([value]);
        return true;
    }
    match value {
        Value::Object(map) => map
            .get(&parts[0])
            .map(|v| walk_path(v, &parts[1..], output))
            .unwrap_or(false),
        Value::Array(a) => {
            let mut found = a.values.is_empty();
            for item in &a.values {
                found |= walk_path(item, parts, output);
            }
            found
        }
        _ => false,
    }
}

pub(crate) fn walk_query_steps<'a>(
    value: &'a Value,
    steps: &[PathStep],
    output: &mut impl Extend<&'a Value>,
) -> bool {
    let Some(step) = steps.first() else {
        output.extend([value]);
        return true;
    };
    match step {
        PathStep::Field(name) => match value {
            Value::Object(map) => map
                .get(name)
                .is_some_and(|value| walk_query_steps(value, &steps[1..], output)),
            Value::Array(array) => {
                let mut found = array.values.is_empty();
                for item in &array.values {
                    found |= walk_query_steps(item, steps, output);
                }
                found
            }
            _ => false,
        },
        PathStep::ArrayIndex(index) => match value {
            Value::Array(array) => array
                .values
                .get(*index as usize)
                .is_some_and(|value| walk_query_steps(value, &steps[1..], output)),
            _ => false,
        },
        PathStep::EachElement => match value {
            Value::Array(array) => {
                let mut found = array.values.is_empty();
                for item in &array.values {
                    found |= walk_query_steps(item, &steps[1..], output);
                }
                found
            }
            _ => false,
        },
    }
}

pub(crate) fn data_values<'a>(value: &'a Value, parts: &[String]) -> Option<Vec<&'a Value>> {
    data_values_small(value, parts).map(SmallVec::into_vec)
}

pub(crate) fn data_values_small<'a>(
    value: &'a Value,
    parts: &[String],
) -> Option<SmallVec<[&'a Value; 1]>> {
    let mut selected = SmallVec::new();
    walk_path(value, parts, &mut selected).then_some(selected)
}

pub(crate) fn select_values<'a>(
    term: &BoundTerm,
    ctx: &'a RecordContext<'a>,
) -> Result<Option<Vec<&'a Value>>, String> {
    match term.path.root {
        PathRoot::Data => {
            let mut selected = Vec::new();
            let found = if term.query_path.has_explicit_array_step {
                walk_query_steps(ctx.value, &term.query_path.steps, &mut selected)
            } else {
                walk_path(ctx.value, &term.path.segments, &mut selected)
            };
            if found { Ok(Some(selected)) } else { Ok(None) }
        }
        PathRoot::Metadata(field) => {
            // Metadata is evaluated from inventory without source parsing.
            // The owned value is handled separately in `eval_term`.
            let _ = field;
            Ok(Some(Vec::new()))
        }
        PathRoot::Backlinks => Err("backlink path requires relation binding".into()),
    }
}

use super::*;
