//! Source framing is format-specific; indexing consumes one extracted record at a time.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::inventory::{ByteRange, SourceRevision};
use crate::model::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    Json,
    JsonLines,
}

#[derive(Clone, Debug)]
pub struct TextSegment {
    pub path: Vec<String>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct ExtractedRecord {
    pub value: Value,
    pub text: Vec<TextSegment>,
}

pub trait SourceReader: Send + Sync {
    fn name(&self) -> &'static str;
    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String>;
    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String>;
}

pub struct JsonReader;
pub struct JsonLinesReader;

fn parse_json(bytes: &[u8]) -> Result<ExtractedRecord, String> {
    let json: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let value = Value::from_json(json)?;
    if !matches!(value, Value::Object(_)) {
        return Err("record root must be an object".into());
    }
    let mut text = Vec::new();
    collect_strings(&value, &mut Vec::new(), &mut text);
    Ok(ExtractedRecord { value, text })
}

fn collect_strings(value: &Value, path: &mut Vec<String>, output: &mut Vec<TextSegment>) {
    match value {
        Value::String(s) => output.push(TextSegment {
            path: path.clone(),
            text: s.clone(),
        }),
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

impl SourceReader for JsonReader {
    fn name(&self) -> &'static str {
        "json"
    }
    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        if file.iter().all(u8::is_ascii_whitespace) {
            return Ok(Vec::new());
        }
        Ok(vec![ByteRange::checked(
            0,
            file.len() as u64,
            file.len() as u64,
        )?])
    }
    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        parse_json(bytes)
    }
}

impl SourceReader for JsonLinesReader {
    fn name(&self) -> &'static str {
        "jsonl"
    }
    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        let mut result = Vec::new();
        let mut start = 0usize;
        for (i, byte) in file.iter().enumerate() {
            if *byte == b'\n' {
                if !file[start..i].iter().all(u8::is_ascii_whitespace) {
                    result.push(ByteRange::checked(
                        start as u64,
                        i as u64,
                        file.len() as u64,
                    )?);
                }
                start = i + 1;
            }
        }
        if start < file.len() && !file[start..].iter().all(u8::is_ascii_whitespace) {
            result.push(ByteRange::checked(
                start as u64,
                file.len() as u64,
                file.len() as u64,
            )?);
        }
        Ok(result)
    }
    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        parse_json(bytes)
    }
}

pub fn reader(format: SourceFormat) -> &'static dyn SourceReader {
    static JSON: JsonReader = JsonReader;
    static JSON_LINES: JsonLinesReader = JsonLinesReader;
    match format {
        SourceFormat::Json => &JSON,
        SourceFormat::JsonLines => &JSON_LINES,
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct ExtractionCounters {
    pub framed_records: u64,
    pub record_parses: u64,
}

/// Call `consume` once for every independently parseable record. No index
/// family rereads or reparses the source; builders share the extracted value.
pub fn extract_file<F>(
    path: &Path,
    format: SourceFormat,
    mut consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(ByteRange, ExtractedRecord) -> Result<(), String>,
{
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let source = reader(format);
    let ranges = source
        .frame(&bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut counters = ExtractionCounters::default();
    for range in ranges {
        let part = &bytes[range.start as usize..range.end as usize];
        let record = source
            .parse(part)
            .map_err(|e| format!("{}:{}..{}: {e}", path.display(), range.start, range.end))?;
        counters.record_parses += 1;
        counters.framed_records += 1;
        consume(range, record)?;
    }
    Ok(counters)
}

/// Read only a stored range and reject changed source metadata both before
/// and after access, so old ranges never silently describe new bytes.
pub fn read_record_range(
    path: &Path,
    revision: SourceRevision,
    range: ByteRange,
) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let before = SourceRevision::from_metadata(&file.metadata().map_err(|e| e.to_string())?)?;
    if before != revision {
        return Err("stale source revision".into());
    }
    ByteRange::checked(range.start, range.end, before.size)?;
    let len = usize::try_from(range.len()).map_err(|_| "record range too large")?;
    let mut bytes = vec![0; len];
    file.seek(SeekFrom::Start(range.start))
        .map_err(|e| e.to_string())?;
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let after = SourceRevision::from_metadata(&file.metadata().map_err(|e| e.to_string())?)?;
    if after != revision {
        return Err("stale source revision during read".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_jsonl_ranges_preserve_bytes_and_boundaries() {
        let bytes = "ü\n{\"x\":\"ä\"}\n\n{\"x\":\"b\"}\n".as_bytes();
        // A non-ASCII prefix is deliberately outside the valid records.
        let start = "ü\n".len();
        let valid = &bytes[start..];
        let ranges = JsonLinesReader.frame(valid).unwrap();
        assert_eq!(ranges.len(), 2);
        assert_eq!(
            &valid[ranges[0].start as usize..ranges[0].end as usize],
            "{\"x\":\"ä\"}".as_bytes()
        );
        for range in ranges {
            JsonLinesReader
                .parse(&valid[range.start as usize..range.end as usize])
                .unwrap();
        }
        assert!(JsonLinesReader.frame(b"").unwrap().is_empty());
    }

    #[test]
    fn fused_extraction_parses_once_and_reports_bad_record() {
        let path = std::env::temp_dir().join(format!(
            "objekte-source-{}-{}.jsonl",
            std::process::id(),
            42
        ));
        std::fs::write(&path, b"{\"name\":\"A\"}\n{\"name\":\"B\"}\n").unwrap();
        let mut seen = Vec::new();
        let stats = extract_file(&path, SourceFormat::JsonLines, |range, record| {
            seen.push((range, record.text[0].text.clone()));
            Ok(())
        })
        .unwrap();
        assert_eq!(stats.record_parses, 2);
        assert_eq!(seen.len(), 2);
        std::fs::write(&path, b"{\"name\":\"A\"}\n{oops}\n").unwrap();
        assert!(
            extract_file(&path, SourceFormat::JsonLines, |_, _| Ok(()))
                .unwrap_err()
                .contains("..")
        );
        std::fs::remove_file(path).unwrap();
    }
}
