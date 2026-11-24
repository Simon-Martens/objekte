//! Contracts and helpers shared with external record adapters.
//!
//! Register a collection schema first, then give each adapter a stable name
//! and version with `IndexBuilder::register_records`. Increase the registration
//! version when its interpretation of records changes; increase the separate
//! representation version when its searchable text or value coverage changes.
//! Submit an owned [`Value`](crate::model::Value) or a versioned reader record.
//! The engine validates typed fields against the schema before publication.
//!
//! For searchable text, pass [`TextSegment`](crate::source::TextSegment)
//! values with stable field paths and value boundaries. With no supplied text,
//! retained input derives text from its validated logical strings. A reader
//! registration supplies later projections through
//! [`RecordReader`](crate::records::reader::RecordReader); it must reject a
//! stale [`RecordVersion`](crate::records::RecordVersion), even when the ID
//! still exists with newer content.
//!
//! File provenance is optional and independent of the payload source. Install
//! one [`FileCatalog`](crate::files::FileCatalog), grant a registration link
//! capability, then declare real file revisions and optional checked ranges
//! inside a batch. A linked retained value remains readable if its file moves.
//! See the runnable `record_ingestion_public` example for all three modes.

pub mod helpers;
