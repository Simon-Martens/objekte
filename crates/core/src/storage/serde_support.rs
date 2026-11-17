use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hash};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub(crate) mod btree_map_sequence {
    use super::*;

    pub(crate) fn serialize<K, V, S>(
        map: &BTreeMap<K, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        K: Serialize,
        V: Serialize,
        S: Serializer,
    {
        serializer.collect_seq(map.iter())
    }

    pub(crate) fn deserialize<'de, K, V, D>(
        deserializer: D,
    ) -> Result<BTreeMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Vec::<(K, V)>::deserialize(deserializer).map(BTreeMap::from_iter)
    }
}

pub(crate) mod hash_map_sequence {
    use super::*;

    pub(crate) fn serialize<K, V, H, S>(
        map: &HashMap<K, V, H>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        K: Serialize,
        V: Serialize,
        H: BuildHasher,
        S: Serializer,
    {
        serializer.collect_seq(map.iter())
    }

    pub(crate) fn deserialize<'de, K, V, H, D>(
        deserializer: D,
    ) -> Result<HashMap<K, V, H>, D::Error>
    where
        K: Deserialize<'de> + Eq + Hash,
        V: Deserialize<'de>,
        H: BuildHasher + Default,
        D: Deserializer<'de>,
    {
        Vec::<(K, V)>::deserialize(deserializer).map(HashMap::from_iter)
    }
}
