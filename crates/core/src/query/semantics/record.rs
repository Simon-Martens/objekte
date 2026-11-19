pub struct RecordContext<'a> {
    pub value: &'a Value,
    pub source_bytes: Option<&'a [u8]>,
    pub record: &'a RecordEntry,
    pub file: &'a FileEntry,
    pub collection_name: &'a str,
    pub repository_root: &'a Path,
}

pub(crate) fn metadata_value(
    field: MetadataField,
    ctx: &RecordContext<'_>,
) -> Result<Value, String> {
    let number = |n: u64| Value::Number(Number::integer(n as i128));
    Ok(match field {
        MetadataField::RecordId => Value::Id(ctx.record.id.0.to_string()),
        MetadataField::RecordByteStart => number(ctx.record.range.start),
        MetadataField::RecordByteEnd => number(ctx.record.range.end),
        MetadataField::RecordByteLength => number(ctx.record.range.len()),
        MetadataField::FileId => number(ctx.file.id.0 as u64),
        MetadataField::FilePath => {
            Value::String(crate::path_repr::query_path(&ctx.file.relative_path))
        }
        MetadataField::FileSize => number(ctx.file.revision.size),
        MetadataField::FileCreated => ctx
            .file
            .created_ns
            .map(timestamp_value)
            .transpose()?
            .unwrap_or(Value::Null),
        MetadataField::FileModified => timestamp_value(ctx.file.revision.modified_ns)?,
        MetadataField::FileStatus => number(ctx.file.status.code() as u64),
        MetadataField::RecordClass => number(ctx.record.class.code() as u64),
        MetadataField::CollectionName => Value::String(ctx.collection_name.into()),
        MetadataField::RepositoryRoot => {
            Value::String(crate::path_repr::query_path(ctx.repository_root))
        }
    })
}

fn timestamp_value(ns: i128) -> Result<Value, String> {
    time::OffsetDateTime::from_unix_timestamp_nanos(ns)
        .map(Value::Datetime)
        .map_err(|e| e.to_string())
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
