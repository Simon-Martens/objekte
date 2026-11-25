use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::inventory::ByteRange;
use crate::model::Value;

use super::{
    DiagnosticCategory, ExtractedRecord, ReaderDiagnostic, ReaderFailure, SourceReader, TextSegment,
};

pub(crate) struct TinyReader(pub(crate) Arc<AtomicUsize>);

pub(crate) struct OutcomeReader {
    pub(crate) ranges: Vec<ByteRange>,
    pub(crate) parses: Arc<AtomicUsize>,
    pub(crate) fail_framing: bool,
}

impl SourceReader for OutcomeReader {
    fn name(&self) -> &'static str {
        "outcome"
    }

    fn frame(&self, _: &[u8]) -> Result<Vec<ByteRange>, String> {
        Ok(self.ranges.clone())
    }

    fn frame_outcome(&self, file: &[u8]) -> Result<Vec<ByteRange>, ReaderFailure> {
        if self.fail_framing {
            Err(ReaderFailure::Malformed(ReaderDiagnostic::new(
                DiagnosticCategory::Framing,
                "outcome.frame",
                "recoverable framing error",
            )))
        } else {
            self.frame(file).map_err(ReaderFailure::Fatal)
        }
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        self.parse_outcome(bytes)
            .map_err(|failure| format!("{failure:?}"))
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        self.parses.fetch_add(1, Ordering::Relaxed);
        if bytes == b"bad" {
            Err(ReaderFailure::Malformed(ReaderDiagnostic::new(
                DiagnosticCategory::Syntax,
                "outcome.syntax",
                "recoverable record error",
            )))
        } else {
            Ok(ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            })
        }
    }
}

impl SourceReader for TinyReader {
    fn name(&self) -> &'static str {
        "tiny"
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        let mut ranges = Vec::new();
        let mut start = 0;
        for end in 0..=file.len() {
            if end == file.len() || file[end] == b'\n' {
                if end > start {
                    ranges.push(ByteRange::checked(
                        start as u64,
                        end as u64,
                        file.len() as u64,
                    )?);
                }
                start = end + 1;
            }
        }
        Ok(ranges)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let line = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let (title, body) = line.split_once('|').ok_or("missing tiny separator")?;
        let value = Value::Object(std::collections::BTreeMap::from([
            ("title".into(), Value::String(title.into())),
            ("body".into(), Value::String(body.into())),
        ]));
        Ok(ExtractedRecord {
            value,
            text: vec![TextSegment {
                path: vec!["body".into()],
                text: body.into(),
            }],
        })
    }
}

pub(crate) mod framing;
mod records;

pub(crate) use records::{
    DOCUMENT_READER, LINE_READER, SCALAR_READER, borrowed_scan_calls, combined_scan_calls,
};

pub(crate) fn fixture_registry() -> super::registry::SourceRegistry {
    let mut sources = super::registry::SourceRegistry::default();
    sources
        .register("fixture-record", 1, "row", Arc::new(DOCUMENT_READER))
        .unwrap();
    sources
        .register("fixture-scalars", 1, "row", Arc::new(SCALAR_READER))
        .unwrap();
    sources
        .register_file_batched("fixture-lines", 1, "rows", Arc::new(LINE_READER))
        .unwrap();
    sources
}
pub(crate) fn document_format() -> super::SourceFormat {
    super::SourceFormat::registered("fixture-record")
}
pub(crate) fn scalar_format() -> super::SourceFormat {
    super::SourceFormat::registered("fixture-scalars")
}
pub(crate) fn line_format() -> super::SourceFormat {
    super::SourceFormat::registered("fixture-lines")
}

pub(crate) fn document_folder(
    name: impl Into<String>,
    path: impl Into<std::path::PathBuf>,
) -> crate::repository::configuration::CollectionDefinition {
    fixture_document_folder(name, path, document_format())
}

pub(crate) fn decoded_scalar_folder(
    name: impl Into<String>,
    path: impl Into<std::path::PathBuf>,
) -> crate::repository::configuration::CollectionDefinition {
    fixture_document_folder(name, path, scalar_format())
}

fn fixture_document_folder(
    name: impl Into<String>,
    path: impl Into<std::path::PathBuf>,
    format: super::SourceFormat,
) -> crate::repository::configuration::CollectionDefinition {
    use crate::repository::configuration::{CollectionDefinition, IdentityPolicy};
    CollectionDefinition::folder_with_identity(name, path, format, IdentityPolicy::RelativePath)
}

pub(crate) use records::{FixtureText, FixtureValue, fixture_value_from_text};

macro_rules! fixture_value {
    (null) => {
        $crate::model::Value::Null
    };
    ({$($key:literal => $value:tt),* $(,)?}) => {
        $crate::model::Value::Object(
            [$((String::from($key), $crate::source::test_support::fixture_value!($value))),*]
                .into_iter()
                .collect(),
        )
    };
    ([$($item:tt),* $(,)?]) => {
        $crate::model::Value::Array($crate::model::ArrayValue::from_unrestricted(
            vec![$($crate::source::test_support::fixture_value!($item)),*],
        ))
    };
    (($value:expr)) => {
        $crate::source::test_support::FixtureValue::into_fixture_value($value)
    };
}

pub(crate) use fixture_value;
