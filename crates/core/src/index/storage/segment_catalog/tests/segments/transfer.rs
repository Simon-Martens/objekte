//! Independent file-backed segment import and conformance.

use super::*;
use crate::index::storage::component::{ComponentDescriptor, ComponentKind};
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
        let candidates: Arc<[u32]> = shortest
            .iter()
            .filter(|slot| {
                input.contains(slot.0) && lists.iter().all(|list| list.binary_search(slot).is_ok())
            })
            .map(|slot| slot.0)
            .collect();
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
    if !matches!(layout, FileSegmentLayout::Identity(_))
        || source.backing_kind() != RecordBacking::FileRange
        || !source.order_columns()?.is_empty()
    {
        return Ok(None);
    }
    let domain = source.descriptor();
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
                    path: descriptor.path.to_vec(),
                    kind: descriptor.kind.clone(),
                    present: present.into(),
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
    layout.visit_records(source, &mut cancelled, &mut |block| {
        for record in block {
            let RecordOrigin::FileRange(locator) = record.origin else {
                return Err(invalid("file table requires file origins"));
            };
            let is_logical = record.header.class == RecordClass::Logical;
            if is_logical != logical.view().contains(record.destination.0)
                || (score.is_some() && is_logical != scores.contains_key(&record.destination.0))
            {
                return Err(invalid("incorrect logical score coverage"));
            }
            rows.push(Row {
                header: record.header.clone(),
                location: Location::File(locator),
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
    if seen.len() != rows.len() {
        return Err(invalid("incomplete file membership"));
    }
    let metadata = source.source();
    let mut table = TableSegment::seal(
        Header {
            id: domain.id,
            collection: domain.collection,
            adapter: metadata.adapter.to_owned(),
            version: metadata.version,
            registration: metadata.registration,
            backing: RecordBacking::FileRange,
        },
        rows,
        files,
        None,
        false,
        control,
    )?;
    table.score = score;
    table.other_postings = other_postings;
    table.relations = relations;
    table.grams = grams;
    table.raw_covers_logical_fields = source.raw_covers_logical_fields();
    Ok(Some(Arc::new(table)))
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
    assert!(!source.active.contains(RecordSlot(0)));
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
        id: source.id,
        index: native,
        active: source.active.clone(),
        observation: None,
    });
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
    let active = flat_pin.0.active[0].bitmap();
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
