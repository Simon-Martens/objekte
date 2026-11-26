//! Independent file-backed segment import and conformance.

use super::*;
use crate::index::storage::component::{ComponentDescriptor, ComponentKind};
use crate::index::storage::posting::visit_posting_blocks;
use crate::index::storage::segment::transfer::FileSegmentLayout;
use crate::index::storage::segment::{read_slot_members, visit_checked_file_slots};
use crate::index::storage::text::{
    GramChannel, GramDescriptor, GramReader, VisitGramBlock, visit_gram_blocks,
};

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

pub(in crate::index::storage::segment_catalog::tests) fn import_file_segment(
    source: &dyn SegmentReader,
    layout: FileSegmentLayout<'_>,
    cancelled: &mut dyn FnMut() -> bool,
    control: Arc<TableFactory>,
) -> StorageResult<Option<SegmentHandle>> {
    if !matches!(layout, FileSegmentLayout::Identity(_))
        || source.backing_kind() != RecordBacking::FileRange
        || source.relation_count() != 0
        || !source.order_columns()?.is_empty()
    {
        return Ok(None);
    }
    let domain = source.descriptor();
    let mut score = None;
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
            ComponentKind::DeclaredPosting
                if component
                    .path
                    .as_deref()
                    .is_some_and(|path| path == [SCORE])
                    && component.binding.is_none() =>
            {
                let reader = source
                    .posting(&component)?
                    .ok_or_else(|| invalid("missing score reader"))?;
                let descriptor = reader.descriptor();
                if descriptor.kind != &LogicalType::Number
                    || descriptor.coverage.multi
                    || !descriptor.coverage.value_complete
                    || score.is_some()
                {
                    return Ok(None);
                }
                let mut present = Vec::new();
                let mut values = BTreeMap::<_, Vec<u32>>::new();
                visit_posting_blocks(reader, domain.slots, &cancelled, &mut |key, slots| {
                    let output = match key {
                        PostingKey::Presence => &mut present,
                        PostingKey::Value(value @ ScalarKey::Number(_)) => {
                            values.entry(value.clone()).or_default()
                        }
                        _ => return Err(invalid("unsupported score key")),
                    };
                    output.extend(slots.iter().map(|slot| slot.0));
                    Ok(())
                })?;
                score = Some(TablePosting {
                    path: descriptor.path.to_vec(),
                    present: present.into(),
                    keys: values
                        .into_iter()
                        .map(|(key, slots)| (key, slots.into()))
                        .collect(),
                    coverage: descriptor.coverage,
                });
            }
            ComponentKind::Sorting | ComponentKind::Relations => {}
            _ => return Ok(None),
        }
    }
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
    table.grams = grams;
    table.raw_covers_logical_fields = source.raw_covers_logical_fields();
    Ok(Some(Arc::new(table)))
}

#[test]
fn file_segment_round_trip_and_compaction_transfer_logical_blocks() {
    use crate::index::storage::publication::PublishedSegment;
    use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
    use crate::source::test_support::{fixture_registry, line_format};

    let factory = Arc::new(TableFactory::default());
    let root = std::env::temp_dir().join(format!(
        "objekte-file-segment-round-trip-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let retired_path = Path::new("data/a.rows");
    let kept_path = Path::new("data/b.rows");
    std::fs::write(
        root.join(retired_path),
        "(\"score\"=7,\"name\"=\"first file\")\n",
    )
    .unwrap();
    std::fs::write(
        root.join(kept_path),
        "(\"score\"=7,\"name\"=\"a festival\")\n(\"score\"=7,\"name\"=\"a festival\")\n(\"score\"=7,\"name\"=\"a festival\")\n(\"score\"=9,\"name\"=\"last\")\n",
    ).unwrap();
    let mut definition = CollectionDefinition::folder("entries", "data", line_format());
    definition.schema = score_schema();
    let index = Index::start_build_with_factory(
        &root,
        vec![definition],
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
    for transferred in [&table_index, &restored] {
        let view = transferred.view().unwrap();
        equivalent(&view);
        assert_eq!(view.native_index_signature().unwrap(), signature);
        let segment = &view.segments(CollectionId(0)).unwrap()[0];
        assert_eq!(segment.active.len(), 4);
        assert_eq!(segment.descriptor(), source.descriptor());
    }
    let table_pin = table_index.view().unwrap();
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
