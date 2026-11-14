//! Borrowed typed-posting and bigram access over native or packed storage.
//!
//! Query planning uses one interface for a `PostingIndex` and a
//! `FieldPostingsView`. Operations return query-owned bitmaps; the access
//! itself never reconstructs a dictionary or caches decoded postings.

use roaring::RoaringBitmap;

use crate::bigram::BigramIndex;
use crate::model::{LogicalType, Number};
use crate::posting::{PostingIndex, ScalarKey, StructuralKey};
use crate::query::Operator;

use super::bigram::BigramSectionView;
use super::bigram_builder::{BigramBinding, BigramBindingKind};
use super::error::StorageResult;
use super::posting::FieldPostingsView;


/// One borrowed typed posting, native or packed.
#[derive(Clone, Copy)]
pub enum PostingAccess<'a> {
    Native(&'a PostingIndex),
    Packed(&'a FieldPostingsView),
}


impl std::fmt::Debug for PostingAccess<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostingAccess")
            .field("kind", self.kind())
            .field("path", &self.path())
            .field("packed", &matches!(self, Self::Packed(_)))
            .finish()
    }
}


impl<'a> PostingAccess<'a> {
    pub fn path(&self) -> &[String] {
        match self {
            Self::Native(index) => &index.path,
            Self::Packed(index) => index.path(),
        }
    }

    pub fn kind(&self) -> &LogicalType {
        match self {
            Self::Native(index) => &index.kind,
            Self::Packed(index) => index.kind(),
        }
    }

    pub fn multi(&self) -> bool {
        match self {
            Self::Native(index) => index.multi,
            Self::Packed(index) => index.multi(),
        }
    }

    pub fn covered_records(&self) -> u32 {
        match self {
            Self::Native(index) => index.covered_records,
            Self::Packed(index) => index.covered_records(),
        }
    }

    pub fn values_indexed(&self) -> bool {
        match self {
            Self::Native(index) => index.values_indexed,
            Self::Packed(index) => index.values_indexed(),
        }
    }

    pub fn value_complete(&self) -> bool {
        match self {
            Self::Native(index) => index.value_complete,
            Self::Packed(index) => index.value_complete(),
        }
    }

    pub fn length_complete(&self) -> bool {
        match self {
            Self::Native(index) => index.length_complete,
            Self::Packed(index) => index.length_complete(),
        }
    }

    pub fn empty_string_complete(&self) -> bool {
        match self {
            Self::Native(index) => index.empty_string_complete,
            Self::Packed(index) => index.empty_string_complete(),
        }
    }

    pub fn presence(&self) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index.presence.clone()),
            Self::Packed(index) => collect(index.presence()?),
        }
    }

    pub fn nulls(&self) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index.nulls.clone()),
            Self::Packed(index) => collect(index.nulls()?),
        }
    }

    pub fn empty_selections(&self) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index.empty_selections.clone()),
            Self::Packed(index) => collect(index.empty_selections()?),
        }
    }

    pub fn empty_string(&self) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index
                .value_posting(&ScalarKey::String(String::new()))
                .cloned()
                .unwrap_or_default()),
            Self::Packed(index) => collect(index.empty_string()?),
        }
    }

    pub fn structural(&self, key: StructuralKey) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index
                .structural
                .as_ref()
                .and_then(|facts| facts.postings.get(&key))
                .cloned()
                .unwrap_or_default()),
            Self::Packed(index) => Ok(index
                .structural(structural_code(key))?
                .map(collect)
                .transpose()?
                .unwrap_or_default()),
        }
    }

    pub fn distinct_values(&self) -> usize {
        match self {
            Self::Native(index) => index.distinct_values(),
            Self::Packed(index) => index.values().entry_count() as usize,
        }
    }

    pub fn value_cardinality(&self, key: &ScalarKey) -> StorageResult<u64> {
        match self {
            Self::Native(index) => Ok(index.value_cardinality(key)),
            Self::Packed(index) => Ok(index.value(key)?.map_or(0, |view| view.cardinality())),
        }
    }

    /// Exact typed equality, without an input restriction.
    pub fn exact(&self, key: &ScalarKey) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index.value_posting(key).cloned().unwrap_or_default()),
            Self::Packed(index) => index.exact(key),
        }
    }

    /// All presence except nulls and the equal value.
    pub fn not_equal(&self, key: &ScalarKey) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => {
                let mut result = index.presence.clone();
                result -= &index.nulls;
                if let Some(equal) = index.value_posting(key) {
                    result -= equal;
                }
                Ok(result)
            }
            Self::Packed(index) => index.not_equal(key),
        }
    }

    /// Scalar comparison with the engine's same-logical-type boundary.
    pub fn range(&self, key: &ScalarKey, operator: Operator) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => match operator {
                Operator::Eq => Ok(index.value_posting(key).cloned().unwrap_or_default()),
                Operator::Ne => {
                    let mut result = index.presence.clone();
                    result -= &index.nulls;
                    if let Some(equal) = index.value_posting(key) {
                        result -= equal;
                    }
                    Ok(result)
                }
                Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                    Ok(index.union_scalar_range(key, operator, None))
                }
                _ => Ok(RoaringBitmap::new()),
            },
            Self::Packed(index) => index.range(key, operator),
        }
    }

    pub fn length(&self, length: &Number) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native(index) => Ok(index.lengths.get(length).cloned().unwrap_or_default()),
            Self::Packed(index) => Ok(index
                .length(length)?
                .map(collect)
                .transpose()?
                .unwrap_or_default()),
        }
    }

    pub fn for_each_value(
        &self,
        mut visit: impl FnMut(&ScalarKey, &RoaringBitmap) -> Result<(), String>,
    ) -> StorageResult<()> {
        match self {
            Self::Native(index) => index
                .for_each_value(|key, bitmap| visit(key, bitmap))
                .map_err(super::error::StorageError::corrupt)?,
            Self::Packed(index) => {
                for entry in index
                    .values()
                    .range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
                {
                    let (key, value) = entry?;
                    let key = super::scalar::decode_scalar_key(&key)?;
                    let reference = super::record_set::RecordSetRef::decode(&value)?;
                    let bitmap =
                        collect(reference.view(index_arena(index)?, index_domain(index)?)?)?;
                    visit(&key, &bitmap).map_err(super::error::StorageError::corrupt)?;
                }
            }
        }
        Ok(())
    }

    pub fn for_each_length(
        &self,
        mut visit: impl FnMut(&Number, &RoaringBitmap) -> Result<(), String>,
    ) -> StorageResult<()> {
        match self {
            Self::Native(index) => index
                .for_each_length(|length, bitmap| visit(length, bitmap))
                .map_err(super::error::StorageError::corrupt)?,
            Self::Packed(index) => {
                let Some(lengths) = index.lengths() else {
                    return Ok(());
                };
                for entry in lengths.range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded) {
                    let (key, value) = entry?;
                    let ScalarKey::Number(length) = super::scalar::decode_scalar_key(&key)? else {
                        return Err(super::error::StorageError::corrupt(
                            "length dictionary contains a non-number key",
                        ));
                    };
                    let reference = super::record_set::RecordSetRef::decode(&value)?;
                    let bitmap =
                        collect(reference.view(index_arena(index)?, index_domain(index)?)?)?;
                    visit(&length, &bitmap).map_err(super::error::StorageError::corrupt)?;
                }
            }
        }
        Ok(())
    }
}


fn index_domain(index: &FieldPostingsView) -> StorageResult<u32> {
    Ok(index.presence()?.domain_len())
}

fn index_arena<'a>(index: &'a FieldPostingsView) -> StorageResult<&'a [u8]> {
    index.arena()
}

fn collect(view: super::record_set::RecordSetView<'_>) -> StorageResult<RoaringBitmap> {
    let mut bitmap = RoaringBitmap::new();
    view.union_into(&mut bitmap)?;
    Ok(bitmap)
}

fn structural_code(key: StructuralKey) -> u8 {
    match key {
        StructuralKey::AnyNull => 0,
        StructuralKey::NonEmpty => 1,
        StructuralKey::String => 2,
        StructuralKey::Number => 3,
        StructuralKey::Array => 4,
        StructuralKey::Object => 5,
        StructuralKey::EmptyObject => 6,
    }
}


/// One borrowed bigram posting access.
#[derive(Clone)]
pub enum BigramAccess<'a> {
    Native {
        index: &'a BigramIndex,
        binding: BigramBinding,
    },
    Packed {
        section: &'a BigramSectionView,
        binding: BigramBinding,
    },
}


impl std::fmt::Debug for BigramAccess<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BigramAccess")
            .field("binding", &self.binding())
            .field("packed", &matches!(self, Self::Packed { .. }))
            .finish()
    }
}


impl<'a> BigramAccess<'a> {
    pub fn binding(&self) -> &BigramBinding {
        match self {
            Self::Native { binding, .. } | Self::Packed { binding, .. } => binding,
        }
    }

    pub fn alphabet(&self) -> u8 {
        match self {
            Self::Native { index, .. } => match index.alphabet {
                crate::execution_options::BigramAlphabet::Ascii => 0,
                crate::execution_options::BigramAlphabet::Unicode => 1,
            },
            Self::Packed { section, binding } => {
                packed_binding(section, binding).map_or(0, |view| view.alphabet())
            }
        }
    }

    pub fn record_count(&self) -> u32 {
        match self {
            Self::Native { index, .. } => index.record_count,
            Self::Packed { section, binding } => {
                packed_binding(section, binding).map_or(0, |view| view.record_count())
            }
        }
    }

    pub fn normal(&self, gram: u64) -> StorageResult<Option<RoaringBitmap>> {
        match self {
            Self::Native { index, .. } => Ok(index.normal.get(&gram).cloned()),
            Self::Packed { section, binding } => {
                let Some(view) = packed_binding(section, binding) else {
                    return Ok(None);
                };
                Ok(view.normal(gram)?.map(collect).transpose()?)
            }
        }
    }

    pub fn skip(&self, gram: u64) -> StorageResult<Option<RoaringBitmap>> {
        match self {
            Self::Native { index, .. } => Ok(index.skip.get(&gram).cloned()),
            Self::Packed { section, binding } => {
                let Some(view) = packed_binding(section, binding) else {
                    return Ok(None);
                };
                Ok(view.skip(gram)?.map(collect).transpose()?)
            }
        }
    }

    pub fn union_normal(&self, grams: &[u64]) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native { index, .. } => {
                let mut result = RoaringBitmap::new();
                for gram in grams {
                    if let Some(posting) = index.normal.get(gram) {
                        result |= posting;
                    }
                }
                Ok(result)
            }
            Self::Packed { section, binding } => {
                let Some(view) = packed_binding(section, binding) else {
                    return Ok(RoaringBitmap::new());
                };
                view.union_normal(grams)
            }
        }
    }

    pub fn intersect_normal(&self, grams: &[u64]) -> StorageResult<Option<RoaringBitmap>> {
        match self {
            Self::Native { index, .. } => {
                let mut result: Option<RoaringBitmap> = None;
                for gram in grams {
                    let Some(posting) = index.normal.get(gram) else {
                        return Ok(Some(RoaringBitmap::new()));
                    };
                    result = Some(match result {
                        Some(previous) => &previous & posting,
                        None => posting.clone(),
                    });
                }
                Ok(result)
            }
            Self::Packed { section, binding } => {
                let Some(view) = packed_binding(section, binding) else {
                    return Ok(Some(RoaringBitmap::new()));
                };
                view.intersect_normal(grams)
            }
        }
    }

    pub fn intersection_count_normal(&self, grams: &[u64]) -> StorageResult<u64> {
        Ok(self
            .intersect_normal(grams)?
            .map_or(0, |bitmap| bitmap.len()))
    }

    pub fn union_skip(&self, grams: &[u64]) -> StorageResult<RoaringBitmap> {
        match self {
            Self::Native { index, .. } => {
                let mut result = RoaringBitmap::new();
                for gram in grams {
                    if let Some(posting) = index.skip.get(gram) {
                        result |= posting;
                    }
                }
                Ok(result)
            }
            Self::Packed { section, binding } => {
                let Some(view) = packed_binding(section, binding) else {
                    return Ok(RoaringBitmap::new());
                };
                view.union_skip(grams)
            }
        }
    }

    pub fn normal_gram_count(&self) -> u64 {
        match self {
            Self::Native { index, .. } => index.normal.len() as u64,
            Self::Packed { section, binding } => {
                packed_binding(section, binding).map_or(0, |view| view.normal_gram_count())
            }
        }
    }

    pub fn skip_gram_count(&self) -> u64 {
        match self {
            Self::Native { index, .. } => index.skip.len() as u64,
            Self::Packed { section, binding } => {
                packed_binding(section, binding).map_or(0, |view| view.skip_gram_count())
            }
        }
    }
}


fn packed_binding<'a>(
    section: &'a BigramSectionView,
    binding: &BigramBinding,
) -> Option<super::bigram::BigramBindingView<'a>> {
    match binding {
        BigramBinding::Raw => section.binding(BigramBindingKind::Raw),
        BigramBinding::Fallback => section.binding(BigramBindingKind::Fallback),
        BigramBinding::Custom { path, name } => section.custom(path, name),
    }
}
