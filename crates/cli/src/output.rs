//! Versioned NDJSON transport and token-preserving JSON payload compaction.

use std::io::{self, Write};
use std::time::Instant;

use objekte_search_core::inventory::{FileStatus, RecordClass};
use objekte_search_core::model::FileId;
use objekte_search_core::path_repr;
use objekte_search_core::repository::{Payload, Repository, SearchHit};
use serde::de::IgnoredAny;
use serde_json::{Value, json};


/// Ordered event writer whose sequence belongs to the transport, not query rank.
pub struct EventWriter<W: Write> {
    writer: W,
    sequence: u64,
    sink_ns: u128,
}


impl<W: Write> EventWriter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            sequence: 0,
            sink_ns: 0,
        }
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn sink_ns(&self) -> u128 {
        self.sink_ns
    }

    pub fn emit(&mut self, mut object: Value) -> io::Result<()> {
        let started = Instant::now();
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("event sequence overflow"))?;
        let map = object
            .as_object_mut()
            .ok_or_else(|| io::Error::other("event is not an object"))?;
        map.insert("protocol_version".into(), json!(1));
        map.insert("sequence".into(), json!(sequence));
        let encoded = serde_json::to_vec(&object).map_err(io::Error::other)?;
        self.writer.write_all(&encoded)?;
        self.writer.write_all(b"\n")?;
        self.sequence = sequence;
        self.sink_ns += started.elapsed().as_nanos();
        Ok(())
    }

    pub fn emit_hit(
        &mut self,
        repo: &Repository,
        hit: &SearchHit,
        query_id: u64,
        position: usize,
        offset: usize,
    ) -> io::Result<()> {
        let started = Instant::now();
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("event sequence overflow"))?;
        let bytes = match hit.payload.as_ref() {
            Some(Payload::Bytes(bytes)) => bytes.as_slice(),
            _ => {
                return Err(io::Error::other(
                    "core did not return source bytes for a hit",
                ));
            }
        };
        // Logical records were already parsed when this revision was admitted.
        // Only fallback bytes need a syntax check to distinguish a declared
        // validation failure from malformed JSON.
        let valid_json = hit.record_class == RecordClass::Logical
            || serde_json::from_slice::<IgnoredAny>(bytes).is_ok();
        let diagnostic = repo
            .files()
            .diagnostic(FileId(hit.file_id), hit.diagnostic_ref);
        let mut event = json!({
            "protocol_version": 1,
            "type": "hit",
            "sequence": sequence,
            "query_id": query_id,
            "position": position,
            "requested_offset": offset,
            "collection": hit.collection,
            "file": {
                "path": path_repr::query_path(&hit.file_path),
                "record_id": hit.record_id,
                "file_id": hit.file_id,
                "size_bytes": hit.file_size,
                "created_ns": hit.file_created_ns,
                "modified_ns": hit.modified_ns,
            },
            "record": {
                "generation": hit.generation,
                "byte_start": hit.byte_start,
                "byte_end": hit.byte_end,
                "class": record_class(hit.record_class),
                "status": file_status(hit.file_status),
                "diagnostic_ref": hit.diagnostic_ref.0,
                "diagnostic": diagnostic,
                "matched_fields": hit.matched_fields,
            },
            "data_available": valid_json,
        });
        let map = event.as_object_mut().expect("object literal");
        // The source document is streamed separately so its tokens are not
        // rewritten by serde_json's value model or cloned into an event tree.
        map.remove("data");
        let mut prefix = serde_json::to_vec(&event).map_err(io::Error::other)?;
        assert_eq!(prefix.pop(), Some(b'}'));
        self.writer.write_all(&prefix)?;
        self.writer.write_all(b",\"data\":")?;
        if valid_json {
            compact_json(bytes, &mut self.writer)?;
        } else {
            self.writer.write_all(b"null")?;
            match std::str::from_utf8(bytes) {
                Ok(text) => {
                    self.writer
                        .write_all(b",\"raw_encoding\":\"utf8\",\"raw_text\":")?;
                    write_json_string(text, &mut self.writer)?;
                }
                Err(_) => {
                    self.writer
                        .write_all(b",\"raw_encoding\":\"base64\",\"raw_base64\":\"")?;
                    // Base64's streaming adapter avoids a second record-sized
                    // string for malformed binary input.
                    let mut encoder = base64::write::EncoderWriter::new(
                        &mut self.writer,
                        &base64::engine::general_purpose::STANDARD,
                    );
                    encoder.write_all(bytes)?;
                    encoder.finish()?;
                    drop(encoder);
                    self.writer.write_all(b"\"")?;
                }
            }
        }
        self.writer.write_all(b"}\n")?;
        self.sequence = sequence;
        self.sink_ns += started.elapsed().as_nanos();
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        let started = Instant::now();
        self.writer.flush()?;
        self.sink_ns += started.elapsed().as_nanos();
        Ok(())
    }
}


fn record_class(class: RecordClass) -> &'static str {
    match class {
        RecordClass::Logical => "logical",
        RecordClass::RawFallback => "raw_fallback",
    }
}

fn file_status(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Valid => "valid",
        FileStatus::Broken => "broken",
        FileStatus::Unreadable => "unreadable",
    }
}

fn compact_json<W: Write>(bytes: &[u8], writer: &mut W) -> io::Result<()> {
    let mut in_string = false;
    let mut escaped = false;
    let mut start = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte.is_ascii_whitespace() {
            if start < index {
                writer.write_all(&bytes[start..index])?;
            }
            start = index + 1;
        }
    }
    if start < bytes.len() {
        writer.write_all(&bytes[start..])?;
    }
    Ok(())
}

fn write_json_string<W: Write>(text: &str, writer: &mut W) -> io::Result<()> {
    writer.write_all(b"\"")?;
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        let escaped = match ch {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            '\u{08}' => Some("\\b"),
            '\u{0c}' => Some("\\f"),
            c if c < '\u{20}' => None,
            _ => continue,
        };
        if start < index {
            writer.write_all(&text.as_bytes()[start..index])?;
        }
        if let Some(escaped) = escaped {
            writer.write_all(escaped.as_bytes())?;
        } else {
            write!(writer, "\\u{:04x}", ch as u32)?;
        }
        start = index + ch.len_utf8();
    }
    if start < text.len() {
        writer.write_all(&text.as_bytes()[start..])?;
    }
    writer.write_all(b"\"")
}

#[cfg(test)]
#[path = "output.test.rs"]
mod tests;
