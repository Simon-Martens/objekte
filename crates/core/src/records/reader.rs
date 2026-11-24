//! Version-checked logical reads from an application-owned record provider.

use crate::model::Value;
use crate::source::TextSegment;

use super::{RecordKey, RecordVersion};

/// Fields and representations needed by one residual or payload read.
///
/// An empty `fields` list requests the full logical value. A reader may return
/// more fields than requested, but must preserve missing versus explicit null.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecordProjection {
    pub fields: Vec<Vec<String>>,
    pub text: bool,
    pub original_bytes: bool,
}

impl RecordProjection {
    pub fn full() -> Self {
        Self::default()
    }
}

/// One versioned read request. The version belongs to the registration that
/// owns `key`; it is not a publication, segment, or file revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordReadRequest {
    pub key: RecordKey,
    pub expected_version: RecordVersion,
}

/// Authoritative data returned by a provider for a requested version.
#[derive(Clone, Debug)]
pub struct RecordRead {
    pub version: RecordVersion,
    pub value: Value,
    pub text: Vec<TextSegment>,
    pub original_bytes: Option<Vec<u8>>,
}

/// A provider must report a stale token even if a newer body with the same
/// key exists. `Unavailable` is a transient source failure; `Fatal` means the
/// reader cannot complete the request as registered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordReadError {
    ChangedVersion {
        expected: RecordVersion,
        actual: Option<RecordVersion>,
    },
    Unavailable(String),
    Fatal(String),
}

impl std::fmt::Display for RecordReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChangedVersion { expected, actual } => write!(
                formatter,
                "record version changed from {} to {}",
                expected.token(),
                actual.map_or_else(|| "unavailable".into(), |value| value.token().to_string())
            ),
            Self::Unavailable(message) => write!(formatter, "record source unavailable: {message}"),
            Self::Fatal(message) => write!(formatter, "record reader error: {message}"),
        }
    }
}

impl std::error::Error for RecordReadError {}

/// Read a projected record from one registered application source.
///
/// Callbacks run without index queue, catalog, or publication locks. The
/// default batch method preserves request order and returns one result for
/// each request. An override must retain that contract.
pub trait RecordReader: Send + Sync {
    fn read_projected(
        &self,
        key: &RecordKey,
        expected_version: RecordVersion,
        projection: &RecordProjection,
    ) -> Result<RecordRead, RecordReadError>;

    fn read_batch(
        &self,
        requests: &[RecordReadRequest],
        projection: &RecordProjection,
    ) -> Vec<Result<RecordRead, RecordReadError>> {
        requests
            .iter()
            .map(|request| self.read_projected(&request.key, request.expected_version, projection))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CollectionId, RecordId};

    struct FixtureReader;

    impl RecordReader for FixtureReader {
        fn read_projected(
            &self,
            key: &RecordKey,
            expected_version: RecordVersion,
            _: &RecordProjection,
        ) -> Result<RecordRead, RecordReadError> {
            if expected_version != RecordVersion::new(7) {
                return Err(RecordReadError::ChangedVersion {
                    expected: expected_version,
                    actual: Some(RecordVersion::new(7)),
                });
            }
            Ok(RecordRead {
                version: expected_version,
                value: Value::String(key.record.0.to_string()),
                text: Vec::new(),
                original_bytes: None,
            })
        }
    }

    #[test]
    fn default_batch_retains_order_and_distinct_errors() {
        let key = |id| RecordKey::new(CollectionId(2), RecordId::intrinsic(id).unwrap());
        let requests = [
            RecordReadRequest {
                key: key("second"),
                expected_version: RecordVersion::new(7),
            },
            RecordReadRequest {
                key: key("stale"),
                expected_version: RecordVersion::new(6),
            },
            RecordReadRequest {
                key: key("first"),
                expected_version: RecordVersion::new(7),
            },
        ];
        let results = FixtureReader.read_batch(&requests, &RecordProjection::full());
        assert_eq!(results.len(), requests.len());
        assert!(matches!(&results[0], Ok(read) if read.value == Value::String("second".into())));
        assert!(matches!(
            &results[1],
            Err(RecordReadError::ChangedVersion { .. })
        ));
        assert!(matches!(&results[2], Ok(read) if read.value == Value::String("first".into())));
    }
}
