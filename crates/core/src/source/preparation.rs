//! Query-side record projection, validation and logical-size accounting.

use crate::inventory::RecordClass;
use crate::query_execution::streaming::{ExecutionContext, ExecutionErrorKind};
use crate::schema::Schema;
use crate::source::{
    ExtractedRecord, ScanProjection, registry::PreparedSource, text::finalize_extracted_text,
};

/// Decode at most once for verification and the selected logical payload.
/// Query projections retain the build-validated logical contract; a winner
/// requesting logical data uses the complete projected record instead.
pub(crate) fn prepare_query_record(
    record_class: RecordClass,
    source: &PreparedSource,
    schema: &Schema,
    bytes: &[u8],
    projection: &ScanProjection,
    _logical_output: bool,
) -> Result<ExtractedRecord, String> {
    let trusted =
        record_class == RecordClass::Logical && source.reader().supports_trusted_query_extraction();
    if record_class == RecordClass::RawFallback {
        return Err("raw fallback record has no logical query value".into());
    }
    let mut record = if trusted {
        source.reader().parse_for_query_trusted(bytes, projection)?
    } else {
        source.reader().parse_for_query(bytes, projection)?
    };
    if trusted {
        schema.convert_indexed_record(&mut record.value, projection.top_level.as_ref())?;
    } else {
        schema.coerce_projected_record(&mut record.value, projection.top_level.as_ref())?;
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
    record_class: RecordClass,
    source: &PreparedSource,
    schema: &Schema,
    bytes: &[u8],
    projection: &ScanProjection,
    _logical_output: bool,
    execution: Option<&ExecutionContext>,
) -> Result<ExtractedRecord, String> {
    let Some(execution) = execution else {
        return prepare_query_record(
            record_class,
            source,
            schema,
            bytes,
            projection,
            _logical_output,
        );
    };
    let extension = source.reader().uses_extension_query_limits();
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
    if record_class == RecordClass::RawFallback {
        return Err("raw fallback record has no logical query value".into());
    }
    let trusted =
        record_class == RecordClass::Logical && source.reader().supports_trusted_query_extraction();
    let mut record = if trusted {
        source.reader().parse_for_query_trusted(bytes, projection)
    } else {
        source.reader().parse_for_query(bytes, projection)
    }
    .map_err(tag_extension)?;
    if trusted {
        schema
            .convert_indexed_record(&mut record.value, projection.top_level.as_ref())
            .map_err(tag_extension)?;
    } else {
        schema
            .coerce_projected_record(&mut record.value, projection.top_level.as_ref())
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

#[cfg(test)]
mod tests;
