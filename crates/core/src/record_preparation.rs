//! Shared parse, conversion, extracted-text and payload boundary.

use crate::reference::Schema;
use crate::repository::Payload;
use crate::source::{ExtractedRecord, PreparedSource, ScanProjection, finalize_extracted_text};
use crate::streaming::{ExecutionContext, ExecutionErrorKind};
use crate::workload::ReturnMode;

pub(crate) fn finish_build_record(
    schema: &Schema,
    record: &mut ExtractedRecord,
) -> Result<(), String> {
    schema.coerce_record(&mut record.value)?;
    record.value.validate_finite_numbers()?;
    Ok(())
}

/// Decode at most once for verification and the selected logical payload.
/// Query projections retain the build-validated logical contract; a winner
/// requesting logical data uses the complete projected record instead.
pub(crate) fn prepare_query_record(
    source: &PreparedSource,
    schema: &Schema,
    bytes: &[u8],
    projection: &ScanProjection,
    logical_output: bool,
) -> Result<ExtractedRecord, String> {
    let mut record = source.reader().parse_for_query(bytes, projection)?;
    if logical_output {
        schema.coerce_projected_record(&mut record.value, projection.top_level.as_ref())?;
    } else {
        schema.coerce_query_record(&mut record.value, projection.top_level.as_ref())?;
    }
    if projection.collect_text {
        finalize_extracted_text(source.reader(), &mut record, schema.has_converted_fields());
    }
    Ok(record)
}

fn estimated_logical_bytes(record: &ExtractedRecord) -> u64 {
    let mut bytes = 0u64;
    let mut pending = vec![&record.value];
    while let Some(value) = pending.pop() {
        bytes = bytes.saturating_add(std::mem::size_of::<crate::model::Value>() as u64);
        match value {
            crate::model::Value::String(value) | crate::model::Value::Id(value) => {
                bytes = bytes.saturating_add(value.capacity() as u64);
            }
            crate::model::Value::Number(crate::model::Number::ExactDecimal(value)) => {
                bytes = bytes.saturating_add(value.digits.capacity() as u64);
            }
            crate::model::Value::Array(array) => {
                bytes = bytes.saturating_add(
                    (array.values.capacity() * std::mem::size_of::<crate::model::Value>()) as u64,
                );
                pending.extend(&array.values);
            }
            crate::model::Value::Object(fields) => {
                bytes = bytes.saturating_add(
                    (fields.len() * std::mem::size_of::<(String, crate::model::Value)>()) as u64,
                );
                for (name, value) in fields {
                    bytes = bytes.saturating_add(name.capacity() as u64);
                    pending.push(value);
                }
            }
            _ => {}
        }
    }
    for segment in &record.text {
        bytes = bytes
            .saturating_add(std::mem::size_of_val(segment) as u64)
            .saturating_add(segment.text.capacity() as u64);
        for part in &segment.path {
            bytes = bytes.saturating_add(part.capacity() as u64);
        }
    }
    bytes
}

pub(crate) fn prepare_query_record_with_execution(
    source: &PreparedSource,
    schema: &Schema,
    bytes: &[u8],
    projection: &ScanProjection,
    logical_output: bool,
    execution: Option<&ExecutionContext>,
) -> Result<ExtractedRecord, String> {
    let Some(execution) = execution else {
        return prepare_query_record(source, schema, bytes, projection, logical_output);
    };
    let extension = source.is_extension();
    execution.check_string()?;
    if extension
        && source
            .reader()
            .estimate_query_value_bytes(bytes, projection)
            .is_some_and(|estimate| estimate > execution.limits.extension_value_bytes)
    {
        return Err(execution.fail(
            ExecutionErrorKind::Resource,
            "extension record estimate exceeds logical-value allowance",
        ));
    }
    let tag_extension = |message: String| {
        if extension {
            execution.fail(ExecutionErrorKind::Extension, message)
        } else {
            message
        }
    };
    let mut record = source
        .reader()
        .parse_for_query(bytes, projection)
        .map_err(tag_extension)?;
    if logical_output {
        schema
            .coerce_projected_record(&mut record.value, projection.top_level.as_ref())
            .map_err(tag_extension)?;
    } else {
        schema
            .coerce_query_record(&mut record.value, projection.top_level.as_ref())
            .map_err(tag_extension)?;
    }
    if projection.collect_text {
        finalize_extracted_text(source.reader(), &mut record, schema.has_converted_fields());
    }
    execution.check_string()?;
    if extension && estimated_logical_bytes(&record) > execution.limits.extension_value_bytes {
        return Err(execution.fail(
            ExecutionErrorKind::Resource,
            "extension record exceeds logical-value allowance",
        ));
    }
    Ok(record)
}

pub(crate) fn materialize(
    mode: &ReturnMode,
    bytes: Vec<u8>,
    logical: Option<ExtractedRecord>,
) -> Result<Option<Payload>, String> {
    Ok(match mode {
        ReturnMode::Locator => None,
        ReturnMode::Bytes => Some(Payload::Bytes(bytes)),
        ReturnMode::Logical => Some(Payload::Logical(
            logical.ok_or("logical payload was not prepared")?.value,
        )),
    })
}

#[cfg(test)]
#[path = "record_preparation.test.rs"]
mod tests;
