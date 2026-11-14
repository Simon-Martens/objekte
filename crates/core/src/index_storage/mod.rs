//! Immutable packed index storage.
//!
//! The same binary readers operate on resident bytes and read-only mapped
//! objects. Construction seals mutable builders into these packed structures;
//! persistence is an explicit store operation.

pub mod backing;
pub mod dictionary;
pub mod dictionary_builder;
pub mod error;
pub mod format;
pub mod record_set;
pub mod scalar;

pub use backing::{Backing, MappedObject};
pub use dictionary::{DictionaryCursor, DictionaryReader, KeyOrder};
pub use dictionary_builder::build_dictionary;
pub use error::{StorageError, StorageResult};
pub use format::{
    BuiltObject, FORMAT_VERSION, HEADER_LEN, ObjectBuilder, ObjectHeader, ObjectKind, ObjectView,
    SectionDescriptor, SectionKind,
};
pub use record_set::{
    BlockCodec, EncodedRecordSet, RecordSetRef, RecordSetTag, RecordSetView, encode_record_set,
};
pub use scalar::{compare_scalar_keys, decode_scalar_key, encode_scalar_key};

#[cfg(test)]
#[path = "backing.test.rs"]
mod backing_tests;

#[cfg(test)]
#[path = "dictionary.test.rs"]
mod dictionary_tests;

#[cfg(test)]
#[path = "record_set.test.rs"]
mod record_set_tests;

#[cfg(test)]
#[path = "scalar.test.rs"]
mod scalar_tests;

#[cfg(test)]
#[path = "format.test.rs"]
mod format_tests;
