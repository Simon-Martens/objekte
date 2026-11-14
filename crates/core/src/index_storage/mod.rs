//! Immutable packed index storage.
//!
//! The same binary readers operate on resident bytes and read-only mapped
//! objects. Construction seals mutable builders into these packed structures;
//! persistence is an explicit store operation.

pub mod access;
pub mod backing;
pub mod bigram;
pub mod bigram_builder;
pub mod catalogue;
pub mod coverage;
pub mod dictionary;
pub mod dictionary_builder;
pub mod error;
pub mod format;
pub mod manifest;
pub mod native_pack;
pub mod packed_materialize;
pub mod packed_restore;
pub mod posting;
pub mod posting_builder;
pub mod record_set;
pub mod records;
pub mod relations;
pub mod scalar;
pub mod segment;
pub mod segment_builder;
pub mod sort;
pub mod status;
pub mod store;
pub mod task;

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
#[path = "access.test.rs"]
mod access_tests;

#[cfg(test)]
#[path = "backing.test.rs"]
mod backing_tests;

#[cfg(test)]
#[path = "dictionary.test.rs"]
mod dictionary_tests;

#[cfg(test)]
#[path = "index.test.rs"]
mod index_tests;

#[cfg(test)]
#[path = "packed_restore.test.rs"]
mod packed_restore_tests;

#[cfg(test)]
#[path = "posting.test.rs"]
mod posting_tests;

#[cfg(test)]
#[path = "record_set.test.rs"]
mod record_set_tests;

#[cfg(test)]
#[path = "scalar.test.rs"]
mod scalar_tests;

#[cfg(test)]
#[path = "catalogue.test.rs"]
mod catalogue_tests;

#[cfg(test)]
#[path = "content_generation.test.rs"]
mod content_generation_tests;

#[cfg(test)]
#[path = "format.test.rs"]
mod format_tests;

#[cfg(test)]
#[path = "segment.test.rs"]
mod segment_tests;

#[cfg(test)]
#[path = "store.test.rs"]
mod store_tests;
