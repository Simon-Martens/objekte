//! JSON Lines source registration for Objekte.

mod framing;
mod reader;
use framing as jsonl_framing;
pub use reader::{JsonLinesReader, register};
