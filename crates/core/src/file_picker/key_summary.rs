//! Conservative summary of decoded top-level keys in one index snapshot.


/// A small Bloom filter: collisions cost a scan, but never hide a real key.
#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct KeySummary {
    words: [u64; 16],
}


impl KeySummary {
    pub(super) fn saturate(&mut self) {
        self.words.fill(u64::MAX);
    }

    pub(super) fn add(&mut self, key: &str) {
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in key.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let first = hash as usize & 1023;
        let second = (hash >> 32) as usize & 1023;
        self.words[first / 64] |= 1 << (first % 64);
        self.words[second / 64] |= 1 << (second % 64);
    }

    pub(super) fn merge(&mut self, other: Self) {
        for (word, incoming) in self.words.iter_mut().zip(other.words) {
            *word |= incoming;
        }
    }

    pub(super) fn might_contain(&self, key: &str) -> bool {
        let mut probe = Self::default();
        probe.add(key);
        self.words
            .iter()
            .zip(probe.words)
            .all(|(word, mask)| word & mask == mask)
    }
}
