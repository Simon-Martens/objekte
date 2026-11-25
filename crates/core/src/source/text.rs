//! Format-independent logical text extraction.

use crate::model::Value;

use super::{ExtractedRecord, SourceReader, TextDerivation, TextSegment};

pub fn finalize_extracted_text(
    reader: &dyn SourceReader,
    record: &mut ExtractedRecord,
    converted_fields: bool,
) {
    if converted_fields && reader.text_derivation() == TextDerivation::LogicalStringsAfterConversion
    {
        record.text = text_segments(&record.value);
    }
}

#[cfg(test)]
thread_local! {
    static OWNED_COLLECTOR_COPIED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn owned_collector_copied_bytes() -> usize {
    OWNED_COLLECTOR_COPIED_BYTES.with(std::cell::Cell::get)
}

pub(crate) fn collect_strings(
    value: &Value,
    path: &mut Vec<String>,
    output: &mut Vec<TextSegment>,
) {
    match value {
        Value::String(s) | Value::Id(s) => {
            #[cfg(test)]
            OWNED_COLLECTOR_COPIED_BYTES.with(|count| count.set(count.get() + s.len()));
            output.push(TextSegment {
                path: path.clone(),
                text: s.clone(),
            });
        }
        Value::Object(object) => {
            for (name, value) in object {
                path.push(name.clone());
                collect_strings(value, path, output);
                path.pop();
            }
        }
        Value::Array(array) => {
            for value in &array.values {
                collect_strings(value, path, output);
            }
        }
        _ => {}
    }
}

pub fn text_segments(value: &Value) -> Vec<TextSegment> {
    let mut output = Vec::new();
    collect_strings(value, &mut Vec::new(), &mut output);
    output
}

#[cfg(feature = "test-hooks")]
pub(crate) fn visit_logical_strings(value: &Value, emit: &mut dyn FnMut(&str)) {
    let _ = visit_logical_strings_and_empty(value, emit);
}

/// Visit every logical string segment while observing empty arrays in the
/// same traversal. The Boolean is a conservative record-wide structural fact.
pub(crate) fn visit_logical_strings_and_empty(value: &Value, emit: &mut dyn FnMut(&str)) -> bool {
    match value {
        Value::String(text) | Value::Id(text) => {
            emit(text);
            false
        }
        Value::Object(fields) => {
            let mut empty = false;
            for value in fields.values() {
                empty |= visit_logical_strings_and_empty(value, emit);
            }
            empty
        }
        Value::Array(array) => {
            let mut empty = array.values.is_empty();
            for value in &array.values {
                empty |= visit_logical_strings_and_empty(value, emit);
            }
            empty
        }
        _ => false,
    }
}
