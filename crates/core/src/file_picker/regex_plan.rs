//! Regex-to-bigram candidate planning adapted from FFF at d1c7dd47 (MIT).
//! See `../bigram/LICENSE`. FFF's private column evaluator is replaced with
//! the local `BigramFilter::query` API; an unavailable posting means scan all.
use crate::bigram::BigramFilter;
use regex_syntax::hir::{Class, Hir, HirKind};
use smallvec::SmallVec;

const MAX_CLASS_EXPAND: usize = 16;


// stack inlined array padded with 0 and tracked length
#[derive(Clone, Copy, PartialEq, Eq)]
struct InlineArray {
    bytes: [u8; MAX_CLASS_EXPAND],
    len: usize,
}


impl InlineArray {
    const fn new() -> Self {
        Self {
            bytes: [0; MAX_CLASS_EXPAND],
            len: 0,
        }
    }

    fn from_byte(b: u8) -> Self {
        let mut set = Self::new();
        set.push(b);
        set
    }

    /// Append a byte; no-op if already full (callers guard against this).
    fn push(&mut self, b: u8) {
        if self.len < MAX_CLASS_EXPAND {
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }
}


impl std::ops::Deref for InlineArray {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}


#[inline]
fn consec_key(a: u8, b: u8) -> Option<u16> {
    let al = a.to_ascii_lowercase();
    let bl = b.to_ascii_lowercase();
    if (32..=126).contains(&al) && (32..=126).contains(&bl) {
        Some((al as u16) << 8 | bl as u16)
    } else {
        None
    }
}


#[derive(Debug, Clone)]
enum BigramQuery {
    Any,
    /// A consecutive bigram key to look up in the main index.
    Consec(u16),
    /// A skip-1 bigram key to look up in the skip sub-index.
    Skip1(u16),
    /// All children must match (intersect posting lists).
    And(Vec<BigramQuery>),
    /// At least one child must match (union posting lists).
    Or(Vec<BigramQuery>),
}


/// SIMD-friendly bitwise OR of two equal-length bitsets.
#[inline]
fn bitset_or(a: &mut [u64], b: &[u64]) {
    a.iter_mut().zip(b.iter()).for_each(|(x, y)| *x |= *y);
}

/// SIMD-friendly bitwise AND of two equal-length bitsets.
#[inline]
fn bitset_and(a: &mut [u64], b: &[u64]) {
    a.iter_mut().zip(b.iter()).for_each(|(x, y)| *x &= *y);
}


impl BigramQuery {
    fn matches_overlay(&self, keys: &[u16]) -> bool {
        match self {
            Self::Any | Self::Skip1(_) => true,
            Self::Consec(key) => keys.binary_search(key).is_ok(),
            Self::And(children) => children.iter().all(|child| child.matches_overlay(keys)),
            Self::Or(children) => children.iter().any(|child| child.matches_overlay(keys)),
        }
    }

    fn is_any(&self) -> bool {
        matches!(self, BigramQuery::Any)
    }

    fn evaluate(&self, index: &BigramFilter) -> Option<Vec<u64>> {
        match self {
            BigramQuery::Any => None,

            BigramQuery::Consec(key) => index.query(&[(*key >> 8) as u8, *key as u8]),

            BigramQuery::Skip1(key) => index.skip_index()?.query(&[(*key >> 8) as u8, *key as u8]),

            BigramQuery::And(children) => {
                let mut result: Option<Vec<u64>> = None;
                for child in children {
                    if let Some(child_bits) = child.evaluate(index) {
                        result = Some(match result {
                            None => child_bits,
                            Some(mut r) => {
                                bitset_and(&mut r, &child_bits);
                                r
                            }
                        });
                    }
                }
                result
            }

            BigramQuery::Or(children) => {
                if children.is_empty() {
                    return None;
                }
                let mut result: Option<Vec<u64>> = None;
                for child in children {
                    // Any branch can't be filtered -> whole OR can't be filtered
                    let child_bits = child.evaluate(index)?;
                    result = Some(match result {
                        None => child_bits,
                        Some(mut r) => {
                            bitset_or(&mut r, &child_bits);
                            r
                        }
                    });
                }
                result
            }
        }
    }
}


pub(super) struct OverlayQuery(BigramQuery);


impl OverlayQuery {
    pub(super) fn new(pattern: &str) -> Self {
        Self(regex_to_bigram_query(pattern))
    }

    pub(super) fn matches(&self, keys: &[u16]) -> bool {
        self.0.matches_overlay(keys)
    }
}


/// Candidate query and boundary byte sets for a regex fragment.
struct HirInfo {
    query: BigramQuery,
    first: Option<InlineArray>,
    last: Option<InlineArray>,
    can_be_empty: bool,
}


impl HirInfo {
    fn empty() -> Self {
        Self {
            query: BigramQuery::Any,
            first: None,
            last: None,
            can_be_empty: true,
        }
    }
}


/// Build a conservative candidate query from the regex syntax tree.
fn regex_to_bigram_query(pattern: &str) -> BigramQuery {
    let mut parser = regex_syntax::ParserBuilder::new()
        .unicode(false)
        .utf8(false)
        .build();

    let hir = match parser.parse(pattern) {
        Ok(h) => h,
        Err(_) => return BigramQuery::Any,
    };

    decompose(&hir).query
}

fn decompose(hir: &Hir) -> HirInfo {
    let can_be_empty = hir.properties().minimum_len().is_none_or(|n| n == 0);

    match hir.kind() {
        HirKind::Empty => HirInfo::empty(),

        HirKind::Literal(lit) => decompose_literal(lit.0.as_ref()),

        HirKind::Class(class) => {
            let bytes = expand_class(class);
            match bytes {
                Some(b) if !b.is_empty() => HirInfo {
                    query: BigramQuery::Any,
                    first: Some(b),
                    last: Some(b),
                    can_be_empty,
                },
                _ => HirInfo {
                    query: BigramQuery::Any,
                    first: None,
                    last: None,
                    can_be_empty,
                },
            }
        }

        HirKind::Look(_) => HirInfo::empty(),

        HirKind::Repetition(rep) => {
            let inner = decompose(&rep.sub);
            if rep.min == 0 {
                HirInfo {
                    query: BigramQuery::Any,
                    first: inner.first,
                    last: inner.last,
                    can_be_empty: true,
                }
            } else {
                // min >= 1: inner bigrams guaranteed
                let mut qs = Vec::new();
                if !inner.query.is_any() {
                    qs.push(inner.query.clone());
                }
                // min >= 2: cross-boundary between consecutive occurrences
                if rep.min >= 2 {
                    push_cross_consec(&mut qs, inner.last.as_deref(), inner.first.as_deref());
                }
                HirInfo {
                    query: simplify_and(qs),
                    first: inner.first,
                    last: inner.last,
                    can_be_empty,
                }
            }
        }

        HirKind::Capture(cap) => decompose(&cap.sub),

        HirKind::Concat(parts) => decompose_concat(parts),

        HirKind::Alternation(alts) => decompose_alternation(alts),
    }
}

/// Extract bigrams from a literal byte sequence.
fn decompose_literal(bytes: &[u8]) -> HirInfo {
    if bytes.is_empty() {
        return HirInfo::empty();
    }

    let lower: SmallVec<[u8; 64]> = bytes.iter().map(|b| b.to_ascii_lowercase()).collect();

    if lower.len() == 1 {
        let b = lower[0];
        let first = if (32..=126).contains(&b) {
            Some(InlineArray::from_byte(b))
        } else {
            None
        };
        return HirInfo {
            query: BigramQuery::Any,
            first,
            last: first,
            can_be_empty: false,
        };
    }

    let mut qs: Vec<BigramQuery> = Vec::new();

    // Consecutive bigrams
    for w in lower.windows(2) {
        if let Some(k) = consec_key(w[0], w[1]) {
            qs.push(BigramQuery::Consec(k));
        }
    }

    // Skip-1 bigrams from the literal itself
    if lower.len() >= 3 {
        for i in 0..lower.len() - 2 {
            if let Some(k) = consec_key(lower[i], lower[i + 2]) {
                qs.push(BigramQuery::Skip1(k));
            }
        }
    }

    let first_byte = lower[0];
    let last_byte = *lower.last().unwrap();

    HirInfo {
        query: simplify_and(qs),
        first: if (32..=126).contains(&first_byte) {
            Some(InlineArray::from_byte(first_byte))
        } else {
            None
        },
        last: if (32..=126).contains(&last_byte) {
            Some(InlineArray::from_byte(last_byte))
        } else {
            None
        },
        can_be_empty: false,
    }
}

fn decompose_concat(parts: &[Hir]) -> HirInfo {
    if parts.is_empty() {
        return HirInfo::empty();
    }

    let infos: Vec<HirInfo> = parts.iter().map(decompose).collect();
    let mut qs: Vec<BigramQuery> = Vec::new();

    for info in &infos {
        if !info.query.is_any() {
            qs.push(info.query.clone());
        }
    }

    //  Dense cross-boundary between adjacent mandatory parts
    for pair in infos.windows(2) {
        if !pair[0].can_be_empty && !pair[1].can_be_empty {
            push_cross_consec(&mut qs, pair[0].last.as_deref(), pair[1].first.as_deref());
        }
    }

    //  Sparse-1 cross-boundary: across a single 1 byte wide middle part
    if parts.len() >= 3 {
        for i in 0..parts.len() - 2 {
            let left = &infos[i];
            let mid = &parts[i + 1];
            let right = &infos[i + 2];

            let min_len = mid.properties().minimum_len();
            let max_len = mid.properties().maximum_len();
            let is_1byte = min_len == Some(1) && max_len == Some(1);

            if is_1byte && !left.can_be_empty && !right.can_be_empty {
                push_cross_skip1(&mut qs, left.last.as_deref(), right.first.as_deref());
            }
        }
    }

    let first = collect_first(&infos);
    let last = collect_last(&infos);
    let can_be_empty = infos.iter().all(|i| i.can_be_empty);

    HirInfo {
        query: simplify_and(qs),
        first,
        last,
        can_be_empty,
    }
}

fn decompose_alternation(alts: &[Hir]) -> HirInfo {
    if alts.is_empty() {
        return HirInfo::empty();
    }

    let infos: Vec<HirInfo> = alts.iter().map(decompose).collect();
    let query = simplify_or(infos.iter().map(|i| i.query.clone()).collect());
    let first = merge_byte_sets(infos.iter().map(|i| &i.first));
    let last = merge_byte_sets(infos.iter().map(|i| &i.last));
    let can_be_empty = infos.iter().any(|i| i.can_be_empty);

    HirInfo {
        query,
        first,
        last,
        can_be_empty,
    }
}

fn expand_class(class: &Class) -> Option<InlineArray> {
    let mut bytes = InlineArray::new();
    match class {
        Class::Bytes(bc) => {
            for range in bc.ranges() {
                let count = (range.end() as usize) - (range.start() as usize) + 1;
                if bytes.len() + count > MAX_CLASS_EXPAND {
                    return None;
                }

                for b in range.start()..=range.end() {
                    if (32..=126).contains(&b) {
                        let lower = b.to_ascii_lowercase();
                        if !bytes.contains(&lower) {
                            bytes.push(lower);
                        }
                    }
                }
            }
        }
        Class::Unicode(uc) => {
            for range in uc.ranges() {
                let start = range.start() as u32;
                let end = range.end() as u32;
                if start > 127 {
                    continue;
                }
                let ascii_end = end.min(126) as u8;
                let ascii_start = start.max(32) as u8;
                if ascii_start > ascii_end {
                    continue;
                }
                let count = (ascii_end - ascii_start) as usize + 1;
                if bytes.len() + count > MAX_CLASS_EXPAND {
                    return None;
                }
                for b in ascii_start..=ascii_end {
                    let lower = b.to_ascii_lowercase();
                    if !bytes.contains(&lower) {
                        bytes.push(lower);
                    }
                }
            }
        }
    }
    if bytes.is_empty() { None } else { Some(bytes) }
}

/// Push consecutive cross-product bigrams into `qs`.
fn push_cross_consec(qs: &mut Vec<BigramQuery>, last: Option<&[u8]>, first: Option<&[u8]>) {
    if let Some(q) = cross_product(last, first, false) {
        qs.push(q);
    }
}

/// Push skip-1 cross-product bigrams into `qs`.
fn push_cross_skip1(qs: &mut Vec<BigramQuery>, last: Option<&[u8]>, first: Option<&[u8]>) {
    if let Some(q) = cross_product(last, first, true) {
        qs.push(q);
    }
}

fn cross_product(last: Option<&[u8]>, first: Option<&[u8]>, skip: bool) -> Option<BigramQuery> {
    let last = last?;
    let first = first?;
    let n = last.len() * first.len();
    if n == 0 || n > MAX_CLASS_EXPAND * MAX_CLASS_EXPAND {
        return None;
    }

    let mut bigrams: Vec<BigramQuery> = Vec::with_capacity(n);
    for &l in last {
        for &f in first {
            if let Some(k) = consec_key(l, f) {
                let node = if skip {
                    BigramQuery::Skip1(k)
                } else {
                    BigramQuery::Consec(k)
                };
                bigrams.push(node);
            }
        }
    }

    match bigrams.len() {
        0 => None,
        1 => Some(bigrams.into_iter().next().unwrap()),
        _ => Some(simplify_or(bigrams)),
    }
}

fn collect_first(infos: &[HirInfo]) -> Option<InlineArray> {
    let mut result = InlineArray::new();
    for info in infos {
        if let Some(ref bytes) = info.first {
            for &b in bytes.iter() {
                if !result.contains(&b) {
                    if result.len() >= MAX_CLASS_EXPAND {
                        return None;
                    }
                    result.push(b);
                }
            }
        } else if !info.can_be_empty {
            return None;
        }
        if !info.can_be_empty {
            break;
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn collect_last(infos: &[HirInfo]) -> Option<InlineArray> {
    let mut result = InlineArray::new();
    for info in infos.iter().rev() {
        if let Some(ref bytes) = info.last {
            for &b in bytes.iter() {
                if !result.contains(&b) {
                    if result.len() >= MAX_CLASS_EXPAND {
                        return None;
                    }
                    result.push(b);
                }
            }
        } else if !info.can_be_empty {
            return None;
        }
        if !info.can_be_empty {
            break;
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn merge_byte_sets<'a>(iter: impl Iterator<Item = &'a Option<InlineArray>>) -> Option<InlineArray> {
    let mut result = InlineArray::new();
    for opt in iter {
        let bytes = opt.as_ref()?;

        for &b in bytes.iter() {
            if !result.contains(&b) {
                if result.len() >= MAX_CLASS_EXPAND {
                    return None;
                }
                result.push(b);
            }
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn simplify_and(children: Vec<BigramQuery>) -> BigramQuery {
    let mut flat: Vec<BigramQuery> = Vec::new();
    for child in children {
        match child {
            BigramQuery::Any => {}
            BigramQuery::And(inner) => flat.extend(inner),
            other => flat.push(other),
        }
    }
    match flat.len() {
        0 => BigramQuery::Any,
        1 => flat.into_iter().next().unwrap(),
        _ => BigramQuery::And(flat),
    }
}

fn simplify_or(children: Vec<BigramQuery>) -> BigramQuery {
    if children.iter().any(|c| c.is_any()) {
        return BigramQuery::Any;
    }
    let mut flat: Vec<BigramQuery> = Vec::new();
    for child in children {
        match child {
            BigramQuery::Or(inner) => flat.extend(inner),
            other => flat.push(other),
        }
    }
    match flat.len() {
        0 => BigramQuery::Any,
        1 => flat.into_iter().next().unwrap(),
        _ => BigramQuery::Or(flat),
    }
}

pub(super) fn candidates(index: &BigramFilter, pattern: &str) -> Option<Vec<u64>> {
    regex_to_bigram_query(pattern).evaluate(index)
}

#[cfg(test)]
#[path = "regex_plan.test.rs"]
mod tests;
