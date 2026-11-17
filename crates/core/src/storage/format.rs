use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::storage::CachePolicy;
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const ENVELOPE_LEN: usize = 64;
pub const CURRENT_LEN: usize = 128;
pub const OBJECT_MAGIC: [u8; 8] = *b"OBJSTOR1";
pub const CURRENT_MAGIC: [u8; 8] = *b"OBJCURR1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageError {
    UnsupportedPlatform(String),
    UnsupportedFormat(String),
    InvalidFormat(String),
    IncompatibleConfiguration(String),
    UnsupportedComponent(String),
    IncompleteBuild,
    StorageBusy(String),
    StorageConflict(String),
    LimitExceeded { limit: u64, requested: u64 },
    Corrupt { identity: String, message: String },
    MissingObject([u8; 32]),
    Io(String),
    CommitUncertain(String),
}

impl StorageError {
    pub(crate) fn limit(message: impl Into<String>) -> Self {
        Self::LimitExceeded {
            limit: 0,
            requested: message.into().len() as u64,
        }
    }

    pub(crate) fn corrupt(identity: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Corrupt {
            identity: identity.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform(message) => {
                write!(formatter, "unsupported platform: {message}")
            }
            Self::UnsupportedFormat(message) => {
                write!(formatter, "unsupported storage format: {message}")
            }
            Self::InvalidFormat(message) => write!(formatter, "invalid storage format: {message}"),
            Self::IncompatibleConfiguration(message) => {
                write!(formatter, "incompatible storage configuration: {message}")
            }
            Self::UnsupportedComponent(message) => {
                write!(formatter, "unsupported stored component: {message}")
            }
            Self::IncompleteBuild => formatter.write_str("initial construction is incomplete"),
            Self::StorageBusy(message) => write!(formatter, "storage busy: {message}"),
            Self::StorageConflict(message) => write!(formatter, "storage conflict: {message}"),
            Self::LimitExceeded { limit, requested } => {
                write!(
                    formatter,
                    "storage read requires {requested} bytes, limit is {limit}"
                )
            }
            Self::Corrupt { identity, message } => {
                write!(formatter, "storage object {identity} is corrupt: {message}")
            }
            Self::MissingObject(digest) => {
                write!(formatter, "missing storage object {}", hex(digest))
            }
            Self::Io(message) => write!(formatter, "storage I/O: {message}"),
            Self::CommitUncertain(message) => {
                write!(formatter, "storage commit uncertain: {message}")
            }
        }
    }
}

impl std::error::Error for StorageError {}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CacheStatus {
    pub policy: CachePolicyStatus,
    pub resident_bytes: u64,
    pub pinned_bytes: u64,
    pub in_flight_bytes: u64,
    pub hits: u64,
    pub misses: u64,
    pub joins: u64,
    pub evictions: u64,
    pub oversize_bypasses: u64,
    pub loaded_families: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub enum CachePolicyStatus {
    #[default]
    KeepLoaded,
    Budget(u64),
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CacheTrimReport {
    pub evicted_entries: u64,
    pub evicted_bytes: u64,
    pub retained_pinned_bytes: u64,
}

#[derive(Clone)]
pub(crate) struct CacheEntry {
    pub value: CacheValue,
    pub serialized_bytes: u64,
}

#[derive(Clone)]
pub(crate) enum CacheValue {
    Serialized(Arc<[u8]>),
    Decoded(Arc<dyn Any + Send + Sync>),
}

impl fmt::Debug for CacheEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.value {
            CacheValue::Serialized(_) => "serialized",
            CacheValue::Decoded(_) => "decoded",
        };
        formatter
            .debug_struct("CacheEntry")
            .field("kind", &kind)
            .field("serialized_bytes", &self.serialized_bytes)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct StorageCache {
    policy: CachePolicy,
    entries: Arc<Mutex<HashMap<CacheKey, CacheEntry>>>,
    stats: Arc<CacheStats>,
}

pub(crate) type CacheKey = ([u8; 32], u64, u32);

#[derive(Debug, Default)]
pub(crate) struct CacheStats {
    pub resident_bytes: AtomicU64,
    pub pinned_bytes: AtomicU64,
    pub in_flight_bytes: AtomicU64,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub joins: AtomicU64,
    pub evictions: AtomicU64,
    pub oversize_bypasses: AtomicU64,
}

impl StorageCache {
    pub(crate) fn new(policy: CachePolicy) -> Self {
        Self {
            policy,
            entries: Arc::new(Mutex::new(HashMap::new())),
            stats: Arc::new(CacheStats::default()),
        }
    }

    pub(crate) fn get(&self, key: &CacheKey) -> Option<Arc<[u8]>> {
        let entry = self.entries.lock().ok()?.get(key).cloned()?;
        let CacheValue::Serialized(bytes) = entry.value else {
            return None;
        };
        self.stats.hits.fetch_add(1, Ordering::Relaxed);
        Some(bytes)
    }

    pub(crate) fn get_decoded<T>(&self, key: &CacheKey) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        let entry = self.entries.lock().ok()?.get(key).cloned()?;
        let CacheValue::Decoded(value) = entry.value else {
            return None;
        };
        let value = value.downcast::<T>().ok()?;
        self.stats.hits.fetch_add(1, Ordering::Relaxed);
        Some(value)
    }

    pub(crate) fn record_miss(&self) {
        self.stats.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_miss_by(&self, count: u64) {
        self.stats.misses.fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn insert(&self, key: CacheKey, bytes: Arc<[u8]>) {
        let length = bytes.len() as u64;
        self.insert_value(key, CacheValue::Serialized(bytes), length);
    }

    pub(crate) fn insert_decoded<T>(&self, key: CacheKey, value: Arc<T>, bytes: u64)
    where
        T: Any + Send + Sync,
    {
        self.insert_value(key, CacheValue::Decoded(value), bytes);
    }

    fn insert_value(&self, key: CacheKey, value: CacheValue, length: u64) {
        if let CachePolicy::Budget { resident_bytes } = self.policy
            && length > resident_bytes.get()
        {
            self.stats.oversize_bypasses.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        let target = match self.policy {
            CachePolicy::KeepLoaded => u64::MAX,
            CachePolicy::Budget { resident_bytes } => resident_bytes.get(),
        };
        let mut resident = self.stats.resident_bytes.load(Ordering::Acquire);
        if resident.saturating_add(length) > target {
            let mut victims = entries
                .iter()
                .filter(|(candidate, _)| **candidate != key)
                .map(|(candidate, entry)| (*candidate, entry.serialized_bytes))
                .collect::<Vec<_>>();
            victims.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
            for (candidate, size) in victims {
                if resident.saturating_add(length) <= target {
                    break;
                }
                if entries.remove(&candidate).is_some() {
                    resident = resident.saturating_sub(size);
                    self.stats.evictions.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let replaced = entries.insert(
            key,
            CacheEntry {
                value,
                serialized_bytes: length,
            },
        );
        if let Some(previous) = replaced {
            resident = resident.saturating_sub(previous.serialized_bytes);
        }
        resident = resident.saturating_add(length);
        self.stats.resident_bytes.store(resident, Ordering::Release);
    }

    pub(crate) fn trim(&self) -> CacheTrimReport {
        let Ok(mut entries) = self.entries.lock() else {
            return CacheTrimReport::default();
        };
        let evicted_entries = entries.len() as u64;
        let evicted_bytes = entries.values().map(|entry| entry.serialized_bytes).sum();
        entries.clear();
        self.stats.resident_bytes.store(0, Ordering::Release);
        self.stats
            .evictions
            .fetch_add(evicted_entries, Ordering::Relaxed);
        CacheTrimReport {
            evicted_entries,
            evicted_bytes,
            retained_pinned_bytes: self.stats.pinned_bytes.load(Ordering::Acquire),
        }
    }

    pub(crate) fn status(&self) -> CacheStatus {
        CacheStatus {
            policy: match self.policy {
                CachePolicy::KeepLoaded => CachePolicyStatus::KeepLoaded,
                CachePolicy::Budget { resident_bytes } => {
                    CachePolicyStatus::Budget(resident_bytes.get())
                }
            },
            resident_bytes: self.stats.resident_bytes.load(Ordering::Acquire),
            pinned_bytes: self.stats.pinned_bytes.load(Ordering::Acquire),
            in_flight_bytes: self.stats.in_flight_bytes.load(Ordering::Acquire),
            hits: self.stats.hits.load(Ordering::Acquire),
            misses: self.stats.misses.load(Ordering::Acquire),
            joins: self.stats.joins.load(Ordering::Acquire),
            evictions: self.stats.evictions.load(Ordering::Acquire),
            oversize_bypasses: self.stats.oversize_bypasses.load(Ordering::Acquire),
            loaded_families: self.entries.lock().map_or(0, |entries| entries.len()),
        }
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(text, "{byte:02x}").expect("writing to String cannot fail");
    }
    text
}

pub(crate) fn object_file_name(digest: &[u8; 32], kind: u16) -> String {
    let suffix = if matches!(kind, 1 | 12..=15) {
        "meta"
    } else {
        "pack"
    };
    format!("{}.{}", hex(digest), suffix)
}

pub(crate) fn envelope(kind: u16, body: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(ENVELOPE_LEN + body.len());
    output.extend_from_slice(&OBJECT_MAGIC);
    output.extend_from_slice(&1u16.to_le_bytes());
    output.extend_from_slice(&kind.to_le_bytes());
    output.extend_from_slice(&(ENVELOPE_LEN as u32).to_le_bytes());
    output.resize(ENVELOPE_LEN, 0);
    output.extend_from_slice(body);
    output
}

pub(crate) fn checked_body(bytes: &[u8], expected_kind: u16) -> Result<&[u8], StorageError> {
    if bytes.len() < ENVELOPE_LEN || bytes[..8] != OBJECT_MAGIC {
        return Err(StorageError::InvalidFormat(
            "object envelope magic mismatch".into(),
        ));
    }
    let version = u16::from_le_bytes(bytes[8..10].try_into().expect("fixed version bytes"));
    let kind = u16::from_le_bytes(bytes[10..12].try_into().expect("fixed kind bytes"));
    let header = u32::from_le_bytes(bytes[12..16].try_into().expect("fixed header bytes"));
    if version != 1 || kind != expected_kind || header != ENVELOPE_LEN as u32 {
        return Err(StorageError::InvalidFormat(
            "unsupported object envelope version, kind, or header length".into(),
        ));
    }
    if bytes[16..ENVELOPE_LEN].iter().any(|byte| *byte != 0) {
        return Err(StorageError::InvalidFormat(
            "object envelope reserved bytes are not zero".into(),
        ));
    }
    Ok(&bytes[ENVELOPE_LEN..])
}
