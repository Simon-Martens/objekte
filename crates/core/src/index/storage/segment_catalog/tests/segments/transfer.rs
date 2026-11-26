//! Independent file-backed segment import and conformance.

use super::*;
use crate::index::storage::component::{ComponentDescriptor, ComponentKind};
use crate::index::storage::order::COLLATION_VERSION;
use crate::index::storage::order::key_transfer::transfer_order_keys;
use crate::index::storage::posting::visit_posting_blocks;
use crate::index::storage::relation::{
    RelationBlock, RelationFacts, RelationRow, VisitRelationTarget, visit_relation_blocks,
};
use crate::index::storage::segment::transfer::FileSegmentLayout;
use crate::index::storage::segment::{read_slot_members, visit_checked_file_slots};
use crate::index::storage::text::{
    BigramRequest, GramChannel, GramDescriptor, GramReader, VisitGramBlock, visit_gram_blocks,
};
use crate::schema::relations::RelationDefinition;

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum GramBinding {
    Raw,
    Fallback,
    Field(Vec<String>, String),
}

impl GramBinding {
    pub(super) fn from_component(
        component: &ComponentDescriptor<'_>,
    ) -> StorageResult<Option<Self>> {
        Ok(match component.kind {
            ComponentKind::RawText => Some(Self::Raw),
            ComponentKind::FallbackText => Some(Self::Fallback),
            ComponentKind::FieldText => {
                let path = component
                    .path
                    .as_deref()
                    .ok_or_else(|| invalid("gram path missing"))?;
                let binding = component
                    .binding
                    .as_deref()
                    .ok_or_else(|| invalid("gram binding missing"))?;
                Some(Self::Field(path.to_vec(), binding.to_owned()))
            }
            _ => None,
        })
    }

    pub(super) fn component(&self, domain: SegmentDescriptor) -> ComponentDescriptor<'_> {
        let (kind, path, binding) = match self {
            Self::Raw => (ComponentKind::RawText, None, None),
            Self::Fallback => (ComponentKind::FallbackText, None, None),
            Self::Field(path, binding) => (
                ComponentKind::FieldText,
                Some(Cow::Borrowed(path.as_slice())),
                Some(Cow::Borrowed(binding.as_str())),
            ),
        };
        ComponentDescriptor {
            collection: domain.collection,
            segment: domain.id,
            kind,
            path,
            binding,
        }
    }
}

#[derive(Clone)]
pub(super) struct TableGram {
    descriptor: GramDescriptor,
    rows: BTreeMap<(GramChannel, u64), Vec<RecordSlot>>,
}

impl GramReader for TableGram {
    fn descriptor(&self) -> GramDescriptor {
        self.descriptor
    }

    fn members(&self, channel: GramChannel, key: u64) -> StorageResult<SlotSet<'_>> {
        let Some(slots) = self.rows.get(&(channel, key)) else {
            return Ok(SlotSet::Empty);
        };
        SlotSet::from_sorted(slots.iter().map(|slot| slot.0).collect())
            .map_err(|error| invalid(error.to_string()))
    }

    fn visit_blocks(&self, limit: BatchLimit, visit: &mut VisitGramBlock<'_>) -> StorageResult<()> {
        let max = limit
            .fixed_batch_len_after(
                OP,
                size_of::<GramChannel>() + size_of::<u64>(),
                size_of::<RecordSlot>(),
            )?
            .min(2);
        for (&(channel, key), slots) in &self.rows {
            for batch in slots.chunks(max) {
                visit(channel, key, batch)?;
            }
        }
        Ok(())
    }
}

impl crate::index::storage::text::GramStore for Arc<TableFactory> {
    fn accepts(
        &self,
        binding: &crate::index::storage::text::GramBinding,
        _: crate::index::storage::text::BigramAlphabet,
    ) -> bool {
        matches!(binding, crate::index::storage::text::GramBinding::Raw)
    }

    fn start(
        &self,
        _: &crate::index::storage::text::GramBinding,
        alphabet: crate::index::storage::text::BigramAlphabet,
    ) -> StorageResult<Box<dyn crate::index::storage::text::GramWriter>> {
        Ok(Box::new(TableGramWriter {
            control: Arc::clone(self),
            alphabet,
            rows: BTreeMap::new(),
        }))
    }
}

struct TableGramWriter {
    control: Arc<TableFactory>,
    alphabet: crate::index::storage::text::BigramAlphabet,
    rows: BTreeMap<(GramChannel, u64), BTreeSet<RecordSlot>>,
}

impl crate::index::storage::text::GramWriter for TableGramWriter {
    fn observe_worker_pending(&mut self, facts: usize, bytes: usize) {
        self.control
            .gram_worker_peak_facts
            .fetch_max(facts, Ordering::Relaxed);
        self.control
            .gram_worker_peak_bytes
            .fetch_max(bytes, Ordering::Relaxed);
    }

    fn merge_worker(
        &mut self,
        base: u32,
        worker: Box<dyn crate::index::storage::text::GramWriter>,
    ) -> StorageResult<()> {
        let worker: Box<dyn std::any::Any + Send> = worker;
        let worker = worker
            .downcast::<Self>()
            .map_err(|_| invalid("wrong gram writer"))?;
        if self.alphabet != worker.alphabet {
            return Err(invalid("different gram alphabets"));
        }
        for (key, slots) in worker.rows {
            let output = self.rows.entry(key).or_default();
            for slot in slots {
                output.insert(RecordSlot(
                    base.checked_add(slot.0)
                        .ok_or_else(|| invalid("gram slot overflow"))?,
                ));
            }
        }
        Ok(())
    }

    fn push(
        &mut self,
        base: u32,
        facts: &[crate::index::storage::text::GramFact],
    ) -> StorageResult<()> {
        assert!(facts.len() <= 256);
        let batch = self
            .control
            .gram_fact_batches
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        if self
            .control
            .cancel_gram_after_batches
            .load(Ordering::Relaxed)
            == batch
        {
            return Err(StorageError::Cancelled);
        }
        for fact in facts {
            let slot = base
                .checked_add(fact.slot.0)
                .ok_or_else(|| invalid("gram slot overflow"))?;
            self.rows
                .entry((fact.channel, fact.key))
                .or_default()
                .insert(RecordSlot(slot));
        }
        Ok(())
    }

    fn seal(self: Box<Self>, slots: u32) -> StorageResult<Arc<dyn GramReader>> {
        if self
            .rows
            .values()
            .any(|members| members.last().is_some_and(|slot| slot.0 >= slots))
        {
            return Err(invalid("gram slot outside domain"));
        }
        Ok(Arc::new(TableGram {
            descriptor: GramDescriptor {
                alphabet: self.alphabet,
                version: crate::index::storage::text::BIGRAM_VERSION,
                covered_records: slots,
            },
            rows: self
                .rows
                .into_iter()
                .map(|(key, slots)| (key, slots.into_iter().collect()))
                .collect(),
        }))
    }
}

const TEXT_PROVIDER: ProviderId = ProviderId(192);

pub(super) struct TableText<'a> {
    pub(super) raw: &'a TableGram,
    pub(super) domain: DomainId,
    pub(super) control: &'a TableFactory,
}

impl TableText<'_> {
    fn request<'a>(&self, request: AccessRequest<'a>) -> Option<&'a BigramRequest> {
        let AccessRequest::Text(fts) = request else {
            return None;
        };
        (fts.fields.is_empty()
            && fts.raw_operation.is_none()
            && fts.raw_matcher.is_none()
            && matches!(fts.mode, crate::query::FtsMode::Literal)
            && !fts
                .gram_request
                .grams(self.raw.descriptor.alphabet)
                .is_empty())
        .then_some(&fts.gram_request)
    }

    fn lists<'a>(
        &'a self,
        request: &'a BigramRequest,
    ) -> impl Iterator<Item = &'a [RecordSlot]> + 'a {
        request
            .grams(self.raw.descriptor.alphabet)
            .iter()
            .map(|&(skip, key)| {
                let channel = if skip {
                    GramChannel::Skip
                } else {
                    GramChannel::Adjacent
                };
                self.raw
                    .rows
                    .get(&(channel, key))
                    .map(Vec::as_slice)
                    .unwrap_or_default()
            })
    }
}

impl IndexAccessProvider for TableText<'_> {
    fn provider_id(&self) -> ProviderId {
        TEXT_PROVIDER
    }

    fn propose(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()> {
        if input.domain == self.domain
            && let Some(request) = self.request(request)
            && let Some(minimum) = self.lists(request).map(|list| list.len() as u64).min()
        {
            offers.push(AccessOffer::new(
                AccessId {
                    provider: TEXT_PROVIDER,
                    operation: OperationId(0),
                },
                term,
                AccessKind::CandidateMembership,
                Coverage::CandidateOnly,
                ProofStrength::Candidate,
                minimum.min(input.input_len),
                request.gram_count(self.raw.descriptor.alphabet) as f64,
                0.0,
                false,
                false,
            ));
        }
        Ok(())
    }

    fn apply<'a>(
        &self,
        _: TermId,
        request: AccessRequest<'_>,
        choice: AccessId,
        input: SlotInput<'a>,
        execution: &AccessExecution<'_>,
    ) -> AccessResult<Restriction<'a>> {
        execution.check()?;
        if choice.provider != TEXT_PROVIDER || choice.operation != OperationId(0) {
            return Err(AccessError::StaleHandle);
        }
        if input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        let request = self
            .request(request)
            .ok_or(AccessError::CapabilityUnavailable)?;
        let lists = self.lists(request).collect::<Vec<_>>();
        let shortest = lists
            .iter()
            .min_by_key(|list| list.len())
            .expect("prepared grams");
        let candidates: Arc<[u32]> = input
            .filter_members(
                shortest
                    .iter()
                    .filter(|slot| lists.iter().all(|list| list.binary_search(slot).is_ok()))
                    .map(|slot| slot.0),
            )?
            .into();
        self.control
            .text_applications
            .fetch_add(1, Ordering::Relaxed);
        self.control
            .text_candidates
            .fetch_add(candidates.len(), Ordering::Relaxed);
        Ok(Restriction::candidate(
            input.replace_slots(SlotSet::from_sorted(candidates)?),
        ))
    }
}

#[derive(Clone)]
pub(super) struct TableRelation {
    declaration: RelationDefinition,
    rows: Vec<(RelationFacts, Vec<(String, Vec<u32>)>)>,
    reverse: BTreeMap<String, Arc<[u32]>>,
    edges: usize,
}

impl crate::index::storage::relation::RelationStore for Arc<TableFactory> {
    fn accepts(&self, _: &RelationDefinition) -> bool {
        true
    }

    fn start(
        &self,
        declaration: &RelationDefinition,
    ) -> StorageResult<Box<dyn crate::index::storage::relation::RelationWriter>> {
        Ok(Box::new(TableRelationWriter {
            declaration: declaration.clone(),
            control: Arc::clone(self),
            facts: BTreeMap::new(),
            targets: BTreeMap::new(),
        }))
    }
}

struct TableRelationWriter {
    declaration: RelationDefinition,
    control: Arc<TableFactory>,
    facts: BTreeMap<u32, RelationFacts>,
    targets: BTreeMap<u32, Vec<(String, Vec<u32>)>>,
}

impl crate::index::storage::relation::RelationWriter for TableRelationWriter {
    fn merge_worker(
        &mut self,
        base: u32,
        worker: Box<dyn crate::index::storage::relation::RelationWriter>,
    ) -> StorageResult<()> {
        let worker: Box<dyn std::any::Any + Send> = worker;
        let worker = worker
            .downcast::<Self>()
            .map_err(|_| invalid("wrong relation writer"))?;
        if self.declaration != worker.declaration {
            return Err(invalid("different relation declarations"));
        }
        let shift = |slot: u32| {
            base.checked_add(slot)
                .ok_or_else(|| invalid("relation slot overflow"))
        };
        for (slot, facts) in worker.facts {
            if self.facts.insert(shift(slot)?, facts).is_some() {
                return Err(invalid("duplicate relation source row"));
            }
        }
        for (slot, targets) in worker.targets {
            if self.targets.insert(shift(slot)?, targets).is_some() {
                return Err(invalid("duplicate relation target row"));
            }
        }
        Ok(())
    }

    fn push(
        &mut self,
        base: u32,
        facts: &[crate::index::storage::relation::RelationFact],
    ) -> StorageResult<()> {
        use crate::index::storage::relation::RelationFactKind;

        assert!(facts.len() <= 128);
        self.control
            .relation_fact_batches
            .fetch_add(1, Ordering::Relaxed);
        for fact in facts {
            let slot = base
                .checked_add(fact.slot.0)
                .ok_or_else(|| invalid("relation slot overflow"))?;
            match &fact.kind {
                RelationFactKind::Source(facts) => {
                    if self.facts.insert(slot, *facts).is_some() {
                        return Err(invalid("duplicate relation source row"));
                    }
                }
                RelationFactKind::Target { id, positions } => {
                    self.targets
                        .entry(slot)
                        .or_default()
                        .push((id.clone(), positions.clone()));
                }
            }
        }
        Ok(())
    }

    fn seal(mut self: Box<Self>, slots: u32) -> StorageResult<Arc<dyn RelationReader>> {
        if self.facts.len() != slots as usize
            || self
                .targets
                .last_key_value()
                .is_some_and(|(&slot, _)| slot >= slots)
        {
            return Err(invalid("relation slot coverage mismatch"));
        }
        let mut reverse = BTreeMap::<String, BTreeSet<u32>>::new();
        let mut edges = 0;
        let mut rows = Vec::new();
        for slot in 0..slots {
            let facts = self
                .facts
                .remove(&slot)
                .ok_or_else(|| invalid("relation row missing"))?;
            let targets = self.targets.remove(&slot).unwrap_or_default();
            edges += targets.len();
            for (id, _) in &targets {
                reverse.entry(id.clone()).or_default().insert(slot);
            }
            rows.push((facts, targets));
        }
        Ok(Arc::new(TableRelation {
            declaration: self.declaration,
            rows,
            reverse: reverse
                .into_iter()
                .map(|(id, slots)| (id, slots.into_iter().collect()))
                .collect(),
            edges,
        }))
    }
}

impl TableRelation {
    fn import(
        reader: &dyn RelationReader,
        slots: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> StorageResult<Self> {
        if reader.source_slots() != slots {
            return Err(invalid("wrong relation row count"));
        }
        let mut rows = Vec::new();
        let mut reverse = BTreeMap::<String, Vec<u32>>::new();
        let mut edges = 0;
        for source in 0..slots as u32 {
            let mut facts = RelationFacts::default();
            let mut targets = Vec::new();
            visit_relation_blocks(reader, RecordSlot(source), cancelled, &mut |block| {
                match block {
                    RelationBlock::Facts(row) => facts = row,
                    RelationBlock::Target { id, positions } => {
                        let sources = reverse.entry(id.to_owned()).or_default();
                        if sources.last() != Some(&source) {
                            sources.push(source);
                        }
                        targets.push((id.to_owned(), positions.to_vec()));
                        edges += 1;
                    }
                }
                Ok(())
            })?;
            rows.push((facts, targets));
        }
        Ok(Self {
            declaration: reader.declaration().clone(),
            rows,
            reverse: reverse
                .into_iter()
                .map(|(id, slots)| (id, slots.into()))
                .collect(),
            edges,
        })
    }

    pub(super) fn sources(&self, target: &str) -> StorageResult<SlotSet<'_>> {
        let slots = self.reverse.get(target).cloned().unwrap_or_default();
        SlotSet::from_sorted(slots).map_err(|error| invalid(error.to_string()))
    }

    pub(super) fn source_batch(
        &self,
        targets: &std::collections::HashSet<String>,
    ) -> StorageResult<RelationMatches<'_>> {
        let mut lists = 0;
        let mut members = 0;
        let mut slots = BTreeSet::new();
        for target in targets {
            if let Some(sources) = self.reverse.get(target) {
                lists += 1;
                members += sources.len() as u64;
                slots.extend(sources.iter().copied());
            }
        }
        Ok(RelationMatches {
            slots: SlotSet::from_sorted(slots.into_iter().collect())
                .map_err(|error| invalid(error.to_string()))?,
            lists,
            members,
        })
    }
}

impl RelationReader for TableRelation {
    fn declaration(&self) -> &RelationDefinition {
        &self.declaration
    }

    fn source_slots(&self) -> usize {
        self.rows.len()
    }

    fn edge_count(&self) -> usize {
        self.edges
    }

    fn source_facts(&self, source: RecordSlot) -> StorageResult<RelationFacts> {
        self.rows
            .get(source.0 as usize)
            .map(|(facts, _)| *facts)
            .ok_or_else(|| invalid("relation source outside table"))
    }

    fn visit_targets(
        &self,
        source: RecordSlot,
        visit: &mut VisitRelationTarget<'_>,
    ) -> StorageResult<()> {
        let (_, targets) = self
            .rows
            .get(source.0 as usize)
            .ok_or_else(|| invalid("relation source outside table"))?;
        for (id, positions) in targets {
            if visit(id, positions)? {
                break;
            }
        }
        Ok(())
    }

    fn reverse_members(&self, target: &str) -> StorageResult<Option<SlotSet<'_>>> {
        self.sources(target).map(Some)
    }

    fn forward(&self, source: RecordSlot) -> StorageResult<RelationRow<'_>> {
        let (facts, targets) = self
            .rows
            .get(source.0 as usize)
            .ok_or_else(|| invalid("relation source outside table"))?;
        Ok(RelationRow {
            targets: targets
                .iter()
                .map(|(id, positions)| {
                    (
                        Cow::Borrowed(id.as_str()),
                        Cow::Borrowed(positions.as_slice()),
                    )
                })
                .collect(),
            present: facts.present,
            null: facts.null,
            empty: facts.empty,
            unavailable: facts.unavailable,
        })
    }
}

pub(in crate::index::storage::segment_catalog::tests) fn import_file_segment(
    source: &dyn SegmentReader,
    layout: FileSegmentLayout<'_>,
    cancelled: &mut dyn FnMut() -> bool,
    control: Arc<TableFactory>,
) -> StorageResult<Option<SegmentHandle>> {
    if !matches!(layout, FileSegmentLayout::Identity(_)) || !source.composite_orders()?.is_empty() {
        return Ok(None);
    }
    let domain = source.descriptor();
    let order_readers = source.order_columns()?;
    if order_readers
        .iter()
        .any(|reader| !control.accepts(reader.path(), reader.kind()))
    {
        return Ok(None);
    }
    let mut score = None;
    let mut other_postings = BTreeMap::new();
    let mut grams = BTreeMap::new();
    let cell = std::cell::RefCell::new(cancelled);
    let mut cancelled = || cell.borrow_mut()();
    for component in source.components()? {
        if component.collection != domain.collection || component.segment != domain.id {
            return Err(invalid("component domain mismatch"));
        }
        if let Some(key) = GramBinding::from_component(&component)? {
            let reader = source
                .grams(&component)?
                .ok_or_else(|| invalid("missing gram reader"))?;
            let mut table = TableGram {
                descriptor: reader.descriptor(),
                rows: BTreeMap::new(),
            };
            visit_gram_blocks(
                reader,
                domain.slots,
                &cancelled,
                &mut |channel, key, slots| {
                    table
                        .rows
                        .entry((channel, key))
                        .or_default()
                        .extend_from_slice(slots);
                    Ok(())
                },
            )?;
            if grams.insert(key, table).is_some() {
                return Err(invalid("duplicate gram binding"));
            }
            continue;
        }
        match component.kind {
            ComponentKind::DeclaredPosting if component.binding.is_none() => {
                let reader = source
                    .posting(&component)?
                    .ok_or_else(|| invalid("missing declared posting reader"))?;
                let descriptor = reader.descriptor();
                if component.path.as_deref() != Some(descriptor.path) {
                    return Err(invalid("posting component path mismatch"));
                }
                if !matches!(descriptor.kind, LogicalType::Number | LogicalType::Id)
                    || descriptor.coverage.multi
                    || !descriptor.coverage.value_complete
                {
                    return Ok(None);
                }
                let mut present = Vec::new();
                let mut values = BTreeMap::<_, Vec<u32>>::new();
                visit_posting_blocks(reader, domain.slots, &cancelled, &mut |key, slots| {
                    let output = match key {
                        PostingKey::Presence => &mut present,
                        PostingKey::Value(
                            value @ (ScalarKey::Number(_) | ScalarKey::Id(_) | ScalarKey::Null),
                        ) => values.entry(value.clone()).or_default(),
                        _ => return Err(invalid("unsupported fixture posting key")),
                    };
                    output.extend(slots.iter().map(|slot| slot.0));
                    Ok(())
                })?;
                let posting = TablePosting {
                    member_reads: Arc::default(),
                    block_reads: Arc::clone(&control.posting_block_reads),
                    path: descriptor.path.to_vec(),
                    kind: descriptor.kind.clone(),
                    present: present.into(),
                    covered: None,
                    keys: values
                        .into_iter()
                        .map(|(key, slots)| (key, slots.into()))
                        .collect(),
                    coverage: descriptor.coverage,
                };
                if posting.path == [SCORE] && posting.kind == LogicalType::Number {
                    if score.replace(posting).is_some() {
                        return Err(invalid("duplicate score dictionary"));
                    }
                } else if other_postings
                    .insert(posting.path.clone(), posting)
                    .is_some()
                {
                    return Err(invalid("duplicate declared dictionary"));
                }
            }
            ComponentKind::Sorting | ComponentKind::Relations => {}
            _ => return Ok(None),
        }
    }
    let relations = (0..source.relation_count())
        .map(|ordinal| {
            let reader = source
                .relation(ordinal)?
                .ok_or_else(|| invalid("missing relation reader"))?;
            TableRelation::import(reader, domain.slots, &cancelled)
        })
        .collect::<StorageResult<Vec<_>>>()?;
    let mut scores = BTreeMap::new();
    if let Some(score) = &score {
        for (key, slots) in &score.keys {
            for &slot in slots.iter() {
                if scores.insert(slot, key.clone()).is_some() {
                    return Err(invalid("multiple scores for a scalar slot"));
                }
            }
        }
        if !score.present.iter().copied().eq(scores.keys().copied()) {
            return Ok(None);
        }
    }

    let empty_arrays = read_slot_members(source, SegmentSlotFact::EmptyArray)?;
    let logical = read_slot_members(source, SegmentSlotFact::Logical)?;
    let mut rows = Vec::new();
    let mut retained = Vec::new();
    let backing = source.backing_kind();
    layout.visit_records(source, &mut cancelled, &mut |block| {
        for record in block {
            let location = match record.origin {
                RecordOrigin::FileRange(locator) => Location::File(locator),
                RecordOrigin::Retained(link) => {
                    let body = crate::index::storage::segment::transfer::read_retained_body(
                        source,
                        record.header.slot,
                        &|| cell.borrow_mut()(),
                    )?;
                    retained.push(bincode::serialize(&(body.value, body.text)).map_err(
                        |error| {
                            StorageError::invalid(
                                "encode independent retained body",
                                error.to_string(),
                            )
                        },
                    )?);
                    Location::Retained(record.destination.0, link)
                }
                RecordOrigin::Provider { version, file, .. } => Location::Versioned(version, file),
            };
            let is_logical = record.header.class == RecordClass::Logical;
            if is_logical != logical.view().contains(record.destination.0)
                || (score.is_some() && is_logical != scores.contains_key(&record.destination.0))
            {
                return Err(invalid("incorrect logical score coverage"));
            }
            rows.push(Row {
                header: record.header.clone(),
                location,
                facts: RowFacts {
                    score: scores.remove(&record.destination.0),
                    empty_array: empty_arrays.view().contains(record.destination.0),
                },
                version: record.write_version,
            });
        }
        Ok(())
    })?;
    let mut files = BTreeMap::new();
    let mut seen = BTreeSet::new();
    layout.visit_files(source, &mut cancelled, |file| {
        let mut slots = Vec::new();
        visit_checked_file_slots(source, file.id, &mut |block| {
            for &slot in block {
                if !seen.insert(slot) || !matches!(rows[slot.0 as usize].location,
                    Location::File(locator) if locator.file == file.id && locator.revision == file.revision)
                {
                    return Err(invalid("incorrect reverse file membership"));
                }
                slots.push(slot);
            }
            Ok(())
        })?;
        if files.insert(file.id, FileRow { metadata: file.clone(), slots }).is_some() {
            return Err(invalid("duplicate file ID"));
        }
        Ok(())
    })?;
    if backing == RecordBacking::FileRange && seen.len() != rows.len() {
        return Err(invalid("incomplete file membership"));
    }
    let mut orders = HashMap::new();
    for reader in order_readers {
        if reader.key_count() != domain.slots || reader.collation_version() != COLLATION_VERSION {
            return Err(invalid("order descriptor mismatch"));
        }
        let definition = DeclaredOrderDefinition {
            path: reader.path().clone(),
            kind: reader.kind().clone(),
            config: SortConfig {
                locale: reader.locale().to_owned(),
                options: reader.options(),
            },
            collation_version: reader.collation_version().to_owned(),
        };
        let mut writer = control.start(definition)?;
        let mut keys = Vec::with_capacity(ORDER_KEY_BATCH_SIZE);
        let mut base = 0;
        transfer_order_keys(
            reader,
            (0..domain.slots as u32).map(|slot| Ok(RecordSlot(slot))),
            &cancelled,
            |key| {
                keys.push(key);
                if keys.len() == ORDER_KEY_BATCH_SIZE {
                    writer.push_keys(RecordSlot(base), &keys)?;
                    base += keys.len() as u32;
                    keys.clear();
                }
                Ok(())
            },
        )?;
        if !keys.is_empty() {
            writer.push_keys(RecordSlot(base), &keys)?;
        }
        let reader = writer.seal(&rows.as_slice())?;
        if orders.insert(reader.path().clone(), reader).is_some() {
            return Err(invalid("duplicate order"));
        }
    }

    let metadata = source.source();
    let mut table = TableSegment::seal(
        Header {
            id: domain.id,
            collection: domain.collection,
            adapter: metadata.adapter.to_owned(),
            version: metadata.version,
            registration: metadata.registration,
            backing,
        },
        rows,
        files,
        (backing == RecordBacking::Retained).then(|| control.seal_retained_bytes(retained)),
        false,
        control,
    )?;
    table.score = score;
    table.other_postings = other_postings;
    table.relations = relations;
    table.orders = orders;
    table.grams = grams;
    table.raw_covers_logical_fields = source.raw_covers_logical_fields();
    Ok(Some(Arc::new(table)))
}

#[test]
fn selected_gram_worker_bounds_pending_occurrences_and_aborts_failed_writes() {
    use crate::query::{CaseSensitivity, FtsMode, FtsQuery};

    let store = Arc::new(TableFactory::default());
    store.store_grams.store(true, Ordering::Relaxed);
    let build = |factory| {
        let mut builder = Index::builder();
        builder.use_storage_factory(factory);
        let items = builder.collection("items", Schema::default()).unwrap();
        let registration = builder.register_records(items, "gram-worker", 1).unwrap();
        (builder.build().unwrap(), registration)
    };
    let (selected, selected_registration) = build(Arc::new(Arc::clone(&store)));
    let (native, native_registration) = build(crate::index::storage::default_factory());
    let selected_session = selected.ingestion();
    let native_session = native.ingestion();
    let selected_writer = selected_session.writer(selected_registration).unwrap();
    let native_writer = native_session.writer(native_registration).unwrap();
    let diversity = |length| {
        let mut seed = 205u32;
        (0..length)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                char::from(33 + ((seed >> 24) % 90) as u8)
            })
            .collect::<String>()
    };
    let read = || {
        (
            store.gram_worker_peak_facts.swap(0, Ordering::Relaxed),
            store.gram_worker_peak_bytes.swap(0, Ordering::Relaxed),
        )
    };
    let mut measurements = Vec::new();
    let fixtures: [(&str, &dyn Fn(usize) -> String); 2] = [
        ("repeated", &|length| "a".repeat(length)),
        ("diverse", &diversity),
    ];
    for (kind, make_text) in fixtures {
        for length in [64 * 1024, 640 * 1024] {
            let id = format!("{kind}-{length}");
            let text = format!("{} Marker {kind}", make_text(length));
            for writer in [&selected_writer, &native_writer] {
                let mut batch = writer.batch();
                batch
                    .insert(
                        id.clone(),
                        Value::Object(BTreeMap::from([(
                            "title".into(),
                            Value::String(text.clone()),
                        )])),
                    )
                    .unwrap();
                writer.submit(batch).unwrap().wait_published().unwrap();
            }
            let (pending, bytes) = read();
            assert_eq!(pending, 256);
            assert_eq!(
                bytes,
                256 * size_of::<crate::index::storage::text::GramFact>()
            );
            measurements.push((kind, length, pending, bytes));
        }
        let mut query = score_query();
        query.collections = vec!["items".into()];
        query.filter = None;
        query.options.return_mode = ReturnMode::Locator;
        query.fts = Some(FtsQuery {
            literal: format!("Marker {kind}"),
            mode: FtsMode::Literal,
            case_sensitivity: CaseSensitivity::Insensitive,
            fields: Vec::new(),
            pattern_parameter: None,
        });
        let actual = selected.view().unwrap().search(&query).unwrap();
        let expected = native.view().unwrap().search(&query).unwrap();
        assert_eq!(actual.count, Some(2));
        assert_eq!(
            actual
                .hits
                .iter()
                .map(|hit| &hit.record_id)
                .collect::<Vec<_>>(),
            expected
                .hits
                .iter()
                .map(|hit| &hit.record_id)
                .collect::<Vec<_>>(),
        );
    }
    eprintln!("alternate gram worker pending facts and bytes {measurements:?}");

    let before = selected.view().unwrap().revision();
    let next_batch = store.gram_fact_batches.load(Ordering::Relaxed) + 1;
    store
        .cancel_gram_after_batches
        .store(next_batch, Ordering::Relaxed);
    let mut batch = selected_writer.batch();
    batch
        .insert(
            "cancelled",
            Value::Object(BTreeMap::from([(
                "title".into(),
                Value::String("a".repeat(640 * 1024)),
            )])),
        )
        .unwrap();
    let error = selected_writer
        .submit(batch)
        .unwrap()
        .wait_published()
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error}");
    assert_eq!(store.gram_fact_batches.load(Ordering::Relaxed), next_batch);
    assert_eq!(selected.view().unwrap().revision(), before);
    selected.shutdown().unwrap();
    native.shutdown().unwrap();
}

#[test]
fn direct_workers_attach_independent_grams_and_relations() {
    use crate::query::{CaseSensitivity, FtsMode, FtsQuery};
    use crate::schema::FieldDefinition;
    use crate::schema::relations::RelationCardinality;

    let factory = Arc::new(TableFactory::default());
    factory.store_grams.store(true, Ordering::Relaxed);
    factory.store_postings.store(true, Ordering::Relaxed);
    factory.store_relations.store(true, Ordering::Relaxed);
    let mut builder = Index::builder();
    builder.use_storage_factory(Arc::new(Arc::clone(&factory)));
    let mut schema = score_schema();
    schema
        .register_field(FieldDefinition {
            path: vec!["target".into()],
            kind: LogicalType::Id,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema
        .register_relation(RelationDefinition {
            path: vec!["target".into()],
            target_collection: "targets".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("sources".into()),
        })
        .unwrap();
    let entries = builder.collection("entries", schema).unwrap();
    let targets = builder.collection("targets", score_schema()).unwrap();
    let registration = builder
        .register_records(entries, "direct-entries", 1)
        .unwrap();
    let targets_registration = builder
        .register_records(targets, "direct-targets", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let entry_writer = session.writer(registration).unwrap();
    let target_writer = session.writer(targets_registration).unwrap();
    let mut batch = target_writer.batch();
    batch.insert("target", score(9)).unwrap();
    target_writer
        .submit(batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let mut batch = entry_writer.batch();
    for (id, name) in [("a", "scarlet velvet"), ("b", "blue canvas")] {
        let mut row = score(7);
        let Value::Object(fields) = &mut row else {
            unreachable!()
        };
        fields.insert("title".into(), Value::String(name.into()));
        fields.insert("target".into(), Value::Id("target".into()));
        batch.insert(id, row).unwrap();
    }
    entry_writer
        .submit(batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let pin = index.view().unwrap();
    let ids = |view: &crate::lifecycle::IndexView, query: &crate::query::Query| {
        view.search(query)
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>()
    };
    let verify = |view: &crate::lifecycle::IndexView, related: &[&str]| {
        let group = view.segments(entries).unwrap();
        assert_eq!(group.len(), 1);
        let payload = group[0].native_payload();
        assert!(payload.raw.normal.is_empty());
        assert!(payload.raw.skip.is_empty());
        assert_eq!(payload.relations[0].edge_count(), 0);
        let relation = group[0].relation(0).unwrap().unwrap();
        assert!((relation as &dyn std::any::Any).is::<TableRelation>());
        assert_eq!(relation.edge_count(), related.len());
        let component = group[0]
            .components()
            .unwrap()
            .into_iter()
            .find(|component| component.kind == ComponentKind::RawText)
            .unwrap();
        let gram = group[0].grams(&component).unwrap().unwrap();
        assert!((gram as &dyn std::any::Any).is::<TableGram>());
        let mut query = score_query();
        query.collections = vec!["entries".into()];
        query.options.return_mode = ReturnMode::Locator;
        query.filter = Some(crate::query::parse_filter("target.score = 9").unwrap());
        assert_eq!(ids(view, &query), related);
        query.filter = None;
        query.fts = Some(FtsQuery {
            literal: "scarlet velvet".into(),
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            fields: Vec::new(),
            pattern_parameter: None,
        });
        assert_eq!(ids(view, &query), ["a"]);
    };
    verify(&pin, &["a", "b"]);
    assert!(factory.gram_fact_batches.load(Ordering::Relaxed) > 0);
    assert!(factory.posting_fact_batches.load(Ordering::Relaxed) > 0);
    assert!(factory.relation_fact_batches.load(Ordering::Relaxed) > 0);

    let mut batch = entry_writer.batch();
    batch.delete("b").unwrap();
    entry_writer
        .submit(batch)
        .unwrap()
        .wait_published()
        .unwrap();
    assert_eq!(index.compact().unwrap().retired_slots_removed, 1);
    verify(&index.view().unwrap(), &["a"]);
    verify(&pin, &["a", "b"]);
    session.finish().unwrap();
}

#[test]
fn retained_and_provider_transfer_preserves_pinned_records_without_provider_reads() {
    use crate::index::storage::publication::PublishedSegment;
    use crate::index::storage::segment::maintenance::SegmentPartition;
    use crate::query_results::Payload;

    let mut builder = Index::builder();
    let collection = builder.collection("entries", score_schema()).unwrap();
    let registration = builder
        .register_records(collection, "retained-transfer", 1)
        .unwrap();
    let provider = builder
        .register_records(collection, "provider-transfer", 1)
        .unwrap();
    let reader = Arc::new(Reader(AtomicUsize::new(0)));
    builder.register_reader(provider, reader.clone()).unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let retained = session.writer(registration).unwrap();
    let versioned = session.writer(provider).unwrap();
    let mut batch = retained.batch();
    batch.insert("retired", score(2)).unwrap();
    let mut large = score(7);
    let Value::Object(fields) = &mut large else {
        unreachable!()
    };
    fields.insert(
        "body".into(),
        Value::String("retained contents".repeat(12_000)),
    );
    batch.insert("kept", large.clone()).unwrap();
    retained.submit(batch).unwrap().wait_published().unwrap();
    let mut batch = versioned.batch();
    batch
        .insert_from_reader("reader", score(7), RecordVersion::new(3))
        .unwrap();
    versioned.submit(batch).unwrap().wait_published().unwrap();
    let prior = index.view().unwrap();
    let mut batch = retained.batch();
    batch.delete("retired").unwrap();
    retained.submit(batch).unwrap().wait_published().unwrap();
    let pinned = index.view().unwrap();
    let group = pinned.segments(collection).unwrap();
    let factory = Arc::new(TableFactory::default());
    let memory = crate::index::storage::test_factory();
    let mut projected = Vec::new();

    for original in group.iter() {
        let layout = FileSegmentLayout::Identity(&original.active);
        let table = factory
            .import_file_segment(original.as_ref(), layout, &mut || false)
            .unwrap()
            .unwrap();
        assert!((table.as_ref() as &dyn std::any::Any).is::<TableSegment>());
        if original.backing_kind() == RecordBacking::Retained {
            let next_read = factory.retained_part_reads.load(Ordering::Relaxed) + 2;
            factory
                .fail_retained_part
                .store(next_read, Ordering::Relaxed);
            let error = memory.import_file_segment(table.as_ref(), layout, &mut || false);
            assert!(
                matches!(error, Err(error) if error.to_string().contains("injected retained part failure"))
            );
            let before = factory.retained_part_reads.load(Ordering::Relaxed);
            let error = memory.import_file_segment(table.as_ref(), layout, &mut || {
                factory.retained_part_reads.load(Ordering::Relaxed) > before
            });
            assert!(matches!(error, Err(StorageError::Cancelled)));
            assert_eq!(index.view().unwrap().revision(), pinned.revision());
        }

        let restored = memory
            .import_file_segment(table.as_ref(), layout, &mut || false)
            .unwrap()
            .unwrap();
        assert!(!(restored.as_ref() as &dyn std::any::Any).is::<TableSegment>());
        assert_eq!(restored.descriptor(), original.descriptor());
        assert_eq!(
            restored.source().registration,
            original.source().registration
        );
        for slot in 0..original.descriptor().slots as u32 {
            let slot = RecordSlot(slot);
            let header = restored.header(slot).unwrap().unwrap();
            let expected = original.header(slot).unwrap().unwrap();
            assert_eq!(header.slot, expected.slot);
            assert_eq!(header.id, expected.id);
            assert_eq!(header.class, expected.class);
            assert_eq!(header.diagnostic_ref, expected.diagnostic_ref);
            assert_eq!(
                restored.origin(slot).unwrap(),
                original.origin(slot).unwrap()
            );
            assert_eq!(
                restored.write_version(slot).unwrap(),
                original.write_version(slot).unwrap()
            );
        }
        let partition = SegmentPartition {
            files: Default::default(),
            slots: original.active.clone(),
        };
        let dense = memory
            .project_segment_partitions(
                table.as_ref(),
                &[partition],
                pinned.0.catalog.as_ref(),
                &mut || false,
            )
            .unwrap()
            .pop()
            .unwrap();
        assert!(!(dense.as_ref() as &dyn std::any::Any).is::<TableSegment>());
        assert_eq!(dense.descriptor().slots, original.active.len() as usize);
        let active = memory
            .live_records(dense.descriptor().slots, InitialLiveness::All)
            .unwrap();
        projected.push(Arc::new(PublishedSegment {
            version: crate::index::storage::publication::PlacementVersion::new(),
            id: original.id,
            index: dense,
            active,
            observation: None,
        }));
    }

    assert_eq!(reader.0.load(Ordering::Relaxed), 0);
    assert_eq!(factory.retained_reads.load(Ordering::Relaxed), 0);
    assert!(factory.retained_part_reads.load(Ordering::Relaxed) > 5);
    let mut root = (*pinned.0).clone();
    root.replace_segment_group(collection.0 as usize, &projected)
        .unwrap();
    let restored = Index::from_published(root);
    let view = restored.view().unwrap();
    assert_eq!(
        view.get_record("entries", "kept", ReturnMode::Logical)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::Logical(large)),
    );
    assert!(
        view.get_record("entries", "retired", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert!(
        prior
            .get_record("entries", "retired", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    let mut query = score_query();
    query.options.return_mode = ReturnMode::Locator;
    assert_eq!(
        view.search(&query).unwrap().hits,
        pinned.search(&query).unwrap().hits
    );
    restored.shutdown().unwrap();
    session.finish().unwrap();
}

#[test]
fn scoped_queries_open_only_selected_and_reached_directories() {
    use crate::index::storage::segment_catalog::build_catalog;
    use crate::query_execution::streaming::{
        CancellationToken, ResultScope, StreamControl, StreamEvent, StreamOptions, StreamOutcome,
    };
    use crate::repository::configuration::{
        CollectionDefinition, IdentityPolicy, RepositoryOptions,
    };
    use crate::schema::FieldDefinition;
    use crate::schema::relations::RelationCardinality;
    use crate::source::test_support::{fixture_registry, line_format};

    let root = std::env::temp_dir().join(format!("objekte-directory-scope-{}", std::process::id()));
    std::fs::create_dir_all(root.join("entries")).unwrap();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    std::fs::create_dir_all(root.join("unrelated")).unwrap();
    std::fs::write(
        root.join("unrelated/other.rows"),
        "(\"id\"=\"other\",\"score\"=7)\n",
    )
    .unwrap();
    std::fs::write(
        root.join("entries/source.rows"),
        concat!(
            "(\"id\"=\"left\",\"score\"=7,\"target\"=\"yes\")\n",
            "(\"id\"=\"right\",\"score\"=7,\"target\"=\"no\")\n",
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("targets/target.rows"),
        "(\"id\"=\"yes\",\"score\"=9)\n(\"id\"=\"no\",\"score\"=0)\n",
    )
    .unwrap();

    let definition = |name| {
        let mut result = CollectionDefinition::folder(name, name, line_format());
        result.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
        result.schema = score_schema();
        result
    };
    let mut entries = definition("entries");
    entries
        .schema
        .register_field(FieldDefinition {
            path: vec!["target".into()],
            kind: LogicalType::Id,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    entries
        .schema
        .register_relation(RelationDefinition {
            path: vec!["target".into()],
            target_collection: "targets".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("sources".into()),
        })
        .unwrap();
    let first_directory = Arc::new(TableFactory::default());
    let second_directory = Arc::new(TableFactory::default());
    let unrelated_directory = Arc::new(TableFactory::default());
    first_directory
        .decode_placements
        .store(true, Ordering::Relaxed);
    let source = Index::start_build_with_factory(
        &root,
        vec![entries, definition("targets"), definition("unrelated")],
        RepositoryOptions {
            verified_cache_bytes: 1024 * 1024,
            ..Default::default()
        },
        fixture_registry(),
        Arc::new(Arc::clone(&first_directory)),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let pinned = source.view().unwrap();
    let mut revision = (*pinned.0).clone();
    let second_group = pinned.segments(CollectionId(1)).unwrap();
    revision.record_routes[1] = build_catalog(&second_directory, second_group.iter()).unwrap();
    let unrelated_group = pinned.segments(CollectionId(2)).unwrap();
    revision.record_routes[2] =
        build_catalog(&unrelated_directory, unrelated_group.iter()).unwrap();
    unrelated_directory
        .fail_placement
        .store(true, Ordering::Relaxed);
    let index = Index::from_published(revision);
    let view = index.view().unwrap();

    second_directory
        .fail_placement
        .store(true, Ordering::Relaxed);
    let mut query = score_query();
    query.collections = vec!["entries".into()];
    query.options.return_mode = ReturnMode::Locator;
    let cold = view.search(&query).unwrap();
    assert_eq!(cold.count, Some(2));
    let warm = view.search(&query).unwrap();
    assert!(warm.counters.verified_result_hit);
    assert_eq!(cold.hits, warm.hits);
    assert_eq!(view.explain(&query).unwrap().len(), 1);
    let mut streamed = Vec::new();
    let outcome = view
        .search_stream_scoped(
            &query,
            ResultScope::All { offset: 0 },
            StreamOptions::default(),
            &CancellationToken::new(),
            &mut |event| {
                if let StreamEvent::OrderedRows { rows, .. } = event {
                    streamed.extend_from_slice(rows);
                }
                Ok(StreamControl::Continue)
            },
        )
        .unwrap();
    assert!(matches!(outcome, StreamOutcome::Complete(summary) if summary.count == Some(2)));
    assert_eq!(streamed, cold.hits);
    assert!(
        view.get_record("entries", "left", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    assert!(
        view.get_record("entries", "absent", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    let selection = view.select_exact("entries", &query).unwrap();
    let mut facet =
        crate::repository::posting_values::PostingValuesRequest::new("entries", "score", 5);
    facet.selection = Some(selection);
    assert_eq!(view.posting_values(&facet).unwrap().values.len(), 1);
    assert_eq!(second_directory.placement_reads.load(Ordering::Relaxed), 0);

    let mut delta_query = query.clone();
    delta_query.options.sort.clear();
    delta_query.options.count = false;
    let left = view.segments(CollectionId(0)).unwrap();
    let right = view.segments(CollectionId(0)).unwrap();
    assert!(!Arc::ptr_eq(&left[0], &right[0]));
    assert!(left[0].same_snapshot(&right[0]));
    let append = root.join("entries/append.rows");
    std::fs::write(&append, r#"("id"="third","score"=7,"target"="yes")"#).unwrap();
    index.add_file(&append).unwrap();
    let updated = index.view().unwrap();
    let delta = updated
        .search_added_since(&view, &delta_query)
        .unwrap()
        .unwrap();
    assert_eq!(delta.result.hits.len(), 1);
    assert_eq!(delta.result.hits[0].record_id, "third");
    assert_eq!(second_directory.placement_reads.load(Ordering::Relaxed), 0);

    let replaced = view
        .file_handle_for_path(std::path::Path::new("entries/source.rows"))
        .unwrap()
        .unwrap();
    index.delete_file(replaced).unwrap();
    let changed_prefix = index.view().unwrap();
    assert!(
        changed_prefix
            .search_added_since(&view, &delta_query)
            .unwrap()
            .is_none()
    );
    assert_eq!(second_directory.placement_reads.load(Ordering::Relaxed), 0);

    query.filter = Some(crate::query::parse_filter("target.score = 9").unwrap());
    let error = view.search(&query).unwrap_err();
    assert!(error.contains("injected placement failure"), "{error}");
    assert_eq!(second_directory.placement_reads.load(Ordering::Relaxed), 1);
    assert!(
        view.select_exact("entries", &query)
            .unwrap_err()
            .contains("injected placement failure")
    );
    assert!(
        view.relation_outgoing_page("entries", &["target".into()], "left", 0, 5)
            .unwrap_err()
            .contains("injected placement failure")
    );
    assert!(
        view.relation_backlink_page("targets", "sources", "yes", 0, 5)
            .unwrap_err()
            .contains("injected placement failure")
    );
    second_directory
        .fail_placement
        .store(false, Ordering::Relaxed);
    let related = view.search(&query).unwrap();
    assert_eq!(related.count, Some(1));
    assert_eq!(related.hits[0].record_id, "left");
    assert_eq!(
        view.relation_outgoing_page("entries", &["target".into()], "left", 0, 5)
            .unwrap()
            .total_occurrences,
        1
    );
    assert_eq!(
        view.relation_backlink_page("targets", "sources", "yes", 0, 5)
            .unwrap()
            .total_sources,
        1
    );
    assert_eq!(
        unrelated_directory.placement_reads.load(Ordering::Relaxed),
        0
    );
    query.collections = vec!["targets".into()];
    query.filter = Some(crate::query::parse_filter("@backlinks.sources.score = 7").unwrap());
    let backlinked = view.search(&query).unwrap();
    assert_eq!(
        backlinked
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["no", "yes"]
    );
    first_directory
        .fail_placement
        .store(true, Ordering::Relaxed);
    assert!(
        view.search(&query)
            .unwrap_err()
            .contains("injected placement failure")
    );

    index.shutdown().unwrap();
    source.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_segment_round_trip_and_compaction_transfer_logical_blocks() {
    use crate::index::storage::publication::PublishedSegment;
    use crate::repository::configuration::{
        CollectionDefinition, IdentityPolicy, RepositoryOptions,
    };
    use crate::schema::FieldDefinition;
    use crate::schema::relations::RelationCardinality;
    use crate::source::test_support::{fixture_registry, line_format};

    let factory = Arc::new(TableFactory::default());
    let root = std::env::temp_dir().join(format!(
        "objekte-file-segment-round-trip-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data/entries")).unwrap();
    std::fs::create_dir_all(root.join("data/targets")).unwrap();
    let retired_path = Path::new("data/entries/a.rows");
    let kept_path = Path::new("data/entries/b.rows");
    std::fs::write(
        root.join(retired_path),
        "(\"score\"=7,\"name\"=\"first file\",\"entry\"=\"good\")\n",
    )
    .unwrap();
    std::fs::write(
        root.join(kept_path),
        concat!(
            "(\"score\"=7,\"name\"=\"A FESTIVAL\",\"entry\"=\"good\")\n",
            "(\"score\"=7,\"name\"=\"a festival\",\"entry\"=\"other\")\n",
            "(\"score\"=7,\"name\"=\"a festival\",\"entry\"=\"missing\")\n",
            "(\"score\"=9,\"name\"=\"last\",\"entry\"=null)\n",
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("data/targets/targets.rows"),
        "(\"id\"=\"good\",\"score\"=9)\n(\"id\"=\"other\",\"score\"=0)\n",
    )
    .unwrap();
    let mut definition = CollectionDefinition::folder("entries", "data/entries", line_format());
    definition.schema = score_schema();
    definition.sort_fields = vec![FieldPath::parse("score").unwrap()];
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["entry".into()],
            kind: LogicalType::Id,
            required: true,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    definition
        .schema
        .register_relation(RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "targets".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("entries".into()),
        })
        .unwrap();
    let mut targets = CollectionDefinition::folder("targets", "data/targets", line_format());
    targets.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    targets.schema = score_schema();
    let index = Index::start_build_with_factory(
        &root,
        vec![definition, targets],
        RepositoryOptions::default(),
        fixture_registry(),
        Arc::new(Arc::clone(&factory)),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let before = index.view().unwrap();
    let catalogue = before.file_catalog().unwrap();
    let retired_id = catalogue.file_by_path(retired_path).unwrap().unwrap().id;
    let kept = catalogue.file_by_path(kept_path).unwrap().unwrap();
    let kept_id = kept.id;
    let kept_ids = catalogue.records_for_file(kept_id).unwrap();
    let mut query = score_query();
    query.collections = vec!["entries".into()];
    query.options.return_mode = ReturnMode::Locator;
    assert_eq!(before.search(&query).unwrap().count, Some(4));
    index
        .delete_file(before.file_handle(retired_id).unwrap().unwrap())
        .unwrap();
    let pinned = index.view().unwrap();
    let group = pinned.segments(CollectionId(0)).unwrap();
    let [source] = group.as_slice() else {
        panic!("expected one source segment");
    };
    assert_eq!(source.active.len(), 4);
    assert_eq!(source.descriptor().slots, 5);
    assert!(!source.active.contains(RecordSlot(0)).unwrap());
    let mut ordered_query = query.clone();
    ordered_query.options.sort = vec![SortField::simple("score", true)];
    ordered_query.options.limit = 2;
    let expected_order = pinned.search(&ordered_query).unwrap();
    assert_eq!(expected_order.count, Some(3));
    assert_eq!(expected_order.hits.len(), 2);
    let mut relation_query = query.clone();
    relation_query.filter = Some(crate::query::parse_filter("entry.score = 9").unwrap());
    let expected_relation = pinned.search(&relation_query).unwrap();
    assert_eq!(expected_relation.count, Some(1));
    assert_eq!(
        expected_relation.hits,
        pinned.search_reference(&relation_query).unwrap().hits
    );
    let mut reverse_query = query.clone();
    reverse_query.collections = vec!["targets".into()];
    reverse_query.filter =
        Some(crate::query::parse_filter("@backlinks.entries.score = 7").unwrap());
    let expected_reverse = pinned.search(&reverse_query).unwrap();
    assert_eq!(expected_reverse.count, Some(2));
    let signature = pinned.native_index_signature().unwrap();
    let expected = pinned.search(&query).unwrap();
    assert_eq!(expected.count, Some(3));

    // Detach the registered source files during each transfer.
    let data = root.join("data");
    let detached = root.join("detached-data");
    std::fs::rename(&data, &detached).unwrap();
    let table = factory
        .import_file_segment(
            source.as_ref(),
            FileSegmentLayout::Identity(&source.active),
            &mut || false,
        )
        .unwrap()
        .unwrap();
    assert!((table.as_ref() as &dyn std::any::Any).is::<TableSegment>());
    let published_table = Arc::new(PublishedSegment {
        version: crate::index::storage::publication::PlacementVersion::new(),
        id: source.id,
        index: table,
        active: source.active.clone(),
        observation: None,
    });
    let native = crate::index::storage::test_factory()
        .import_file_segment(
            published_table.as_ref(),
            FileSegmentLayout::Identity(&published_table.active),
            &mut || false,
        )
        .unwrap()
        .unwrap();
    assert!(!(native.as_ref() as &dyn std::any::Any).is::<TableSegment>());
    let published_native = Arc::new(PublishedSegment {
        version: crate::index::storage::publication::PlacementVersion::new(),
        id: source.id,
        index: native,
        active: source.active.clone(),
        observation: None,
    });
    {
        use crate::index::storage::segment::maintenance::SegmentPartition;

        let memory = crate::index::storage::test_factory();
        let make_partition = |slots: ActiveRecords| SegmentPartition {
            files: [kept_id].into_iter().collect(),
            slots,
        };
        let project = |parts: &[SegmentPartition]| {
            let before = factory.posting_block_reads.load(Ordering::Relaxed);
            let projected = memory
                .project_segment_partitions(
                    published_table.as_ref(),
                    parts,
                    pinned.0.catalog.as_ref(),
                    &mut || false,
                )
                .unwrap();
            assert_eq!(projected.len(), parts.len());
            let reads = factory.posting_block_reads.load(Ordering::Relaxed) - before;
            assert!(reads > 0);
            reads
        };
        let one = project(&[make_partition(published_table.active.clone())]);
        let choices = published_table
            .active
            .bitmap()
            .unwrap()
            .iter()
            .map(|slot| {
                make_partition(
                    ActiveRecords::from_bitmap(
                        published_table.descriptor().slots,
                        [slot].into_iter().collect(),
                    )
                    .unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(choices.len(), 4);
        assert_eq!(project(&choices), one);
    }
    let mut flat = pinned.repository().empty_like().unwrap();
    flat.catalog = Arc::clone(&pinned.0.catalog);
    flat.collections[0].base_records = Some(Arc::clone(&published_table.index));
    flat.active[0] = published_table.active.clone();
    let flat = Index::from_repository(flat).unwrap();
    let make_index = |segment: Arc<PublishedSegment>| {
        let mut root = (*pinned.0).clone();
        root.replace_segment_group(0, &[segment]).unwrap();
        Index::from_published(root)
    };
    let table_index = make_index(published_table);
    let restored = make_index(published_native);
    std::fs::rename(&detached, &data).unwrap();
    let kept_version = pinned.file_version(kept_id).unwrap();
    let equivalent = |view: &crate::lifecycle::IndexView| {
        let mut actual = view.search(&query).unwrap();
        assert_eq!(actual.count, expected.count);
        for hit in &mut actual.hits {
            assert_eq!(hit.generation, view.revision().0);
            hit.generation = pinned.revision().0;
        }
        assert_eq!(actual.hits, expected.hits);
        assert_eq!(view.file_record_count(kept_id).unwrap(), Some(4));
        assert_eq!(view.file_version(kept_id).unwrap(), kept_version);
        let catalog = view.file_catalog().unwrap();
        assert_eq!(catalog.records_for_file(kept_id).unwrap(), kept_ids);
        assert!(catalog.file(retired_id).is_err());
    };
    let equivalent_relations = |view: &crate::lifecycle::IndexView| {
        for (query, expected) in [
            (&relation_query, &expected_relation),
            (&reverse_query, &expected_reverse),
            (&ordered_query, &expected_order),
        ] {
            let mut actual = view.search(query).unwrap();
            assert_eq!(actual.count, expected.count);
            let mut reference = view.search_reference(query).unwrap();
            for hit in actual.hits.iter_mut().chain(&mut reference.hits) {
                hit.generation = pinned.revision().0;
            }
            assert_eq!(actual.hits, reference.hits);
            assert_eq!(actual.hits, expected.hits);
        }
    };
    for transferred in [&table_index, &restored] {
        let view = transferred.view().unwrap();
        equivalent(&view);
        equivalent_relations(&view);
        assert_eq!(view.native_index_signature().unwrap(), signature);
        let segment = &view.segments(CollectionId(0)).unwrap()[0];
        assert_eq!(segment.active.len(), 4);
        assert_eq!(segment.descriptor(), source.descriptor());
    }
    assert!(factory.declared_order_builds.load(Ordering::Relaxed) > 0);
    let table_pin = table_index.view().unwrap();
    let text_query = crate::query::Query {
        collections: Vec::new(),
        fts: Some(crate::query::FtsQuery {
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            mode: crate::query::FtsMode::Literal,
            literal: "FESTIVAL".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        }),
        filter: None,
        options: query.options.clone(),
    };
    let text = table_pin.search(&text_query).unwrap();
    let text_reference = table_pin.search_reference(&text_query).unwrap();
    assert_eq!(text.count, Some(1));
    assert_eq!(text.hits, text_reference.hits);
    assert!(factory.text_applications.load(Ordering::Relaxed) > 0);
    assert!(factory.text_candidates.load(Ordering::Relaxed) > text.hits.len());
    let flat_pin = flat.view().unwrap();
    assert!(flat_pin.segments(CollectionId(0)).unwrap().is_empty());
    let collection = &flat_pin.repository().collections()[0];
    assert_eq!(collection.base_logical_count().unwrap(), 5);
    assert_eq!(flat_pin.0.active[0].len(), 4);
    let filter = crate::query::binding::filter::bind_filter(
        query.filter.as_ref().unwrap(),
        &collection.definition.schema,
    )
    .unwrap();
    let estimate =
        crate::query_execution::restrictions::estimate_plan_expr(collection, &filter, 5).unwrap();
    assert!(estimate.exact);
    assert_eq!(estimate.candidates, 4);
    let active = flat_pin.0.active[0].bitmap().unwrap();
    let applications = factory.independent_applications.load(Ordering::Relaxed);
    let plan = crate::query_execution::restrictions::plan_expr_restricted(
        flat_pin.repository(),
        collection,
        &flat_pin.repository().segment_groups().unwrap(),
        &filter,
        &active,
        &active,
        true,
    )
    .unwrap();
    assert!(plan.exact);
    assert_eq!(plan.candidates.unwrap().len(), 3);
    assert_eq!(
        factory.independent_applications.load(Ordering::Relaxed),
        applications + 1,
    );
    let mut order = crate::index::storage::ordered_slots::OrderedSlotCursor::new(
        collection.base_identity_order().unwrap(),
    );
    let mut ids = Vec::new();
    while let Some(slot) = order.next_slot().unwrap() {
        ids.push(collection.base_record(slot).unwrap().id.clone());
    }
    assert_eq!(ids.len(), 5);
    assert!(ids.is_sorted());
    let facet = crate::repository::posting_values::PostingValuesRequest::new("entries", "score", 5);
    let values = flat_pin.posting_values(&facet).unwrap();
    assert_eq!(values.values[0].count, 3);
    assert_eq!(values.values[1].count, 1);
    equivalent(&flat_pin);
    assert!(
        flat_pin
            .get_record("entries", &expected.hits[0].record_id, ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    let old_route = table_pin.0.record_routes[0]
        .locate(&expected.hits[0].record_id)
        .unwrap()
        .unwrap();
    std::fs::rename(&data, &detached).unwrap();
    assert_eq!(table_index.compact().unwrap().retired_slots_removed, 1);
    assert_eq!(flat.compact().unwrap().retired_slots_removed, 1);
    std::fs::rename(&detached, &data).unwrap();
    let compacted = table_index.view().unwrap();
    equivalent(&compacted);
    equivalent_relations(&compacted);
    let compacted_group = compacted.segments(CollectionId(0)).unwrap();
    assert_eq!(compacted_group[0].descriptor().slots, 4);
    assert!(!(compacted_group[0].index.as_ref() as &dyn std::any::Any).is::<TableSegment>());
    let new_route = compacted.0.record_routes[0]
        .locate(&expected.hits[0].record_id)
        .unwrap()
        .unwrap();
    assert_eq!(old_route.route.slot.0, new_route.route.slot.0 + 1);
    equivalent(&table_pin);
    equivalent(&flat_pin);
    let flat_compacted = flat.view().unwrap();
    equivalent(&flat_compacted);
    assert_eq!(
        flat_compacted.segments(CollectionId(0)).unwrap()[0]
            .descriptor()
            .slots,
        4
    );
    flat.shutdown().unwrap();
    table_index.shutdown().unwrap();
    restored.shutdown().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
