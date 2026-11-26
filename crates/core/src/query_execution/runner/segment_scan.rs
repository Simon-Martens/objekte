//! Segment source verification and scanning.

use super::*;
use crate::index::traits::file_catalog::FileCatalogReader;
use crate::records::RecordIdSlot;
use std::sync::atomic::Ordering as AtomicOrdering;

/// Verify one local segment candidate with the narrow source or value consumer.
/// Relation and opaque binding requests stay on the established evaluator.
#[allow(clippy::too_many_arguments)]
pub(super) fn verify_local_segment_record(
    repo: &Repository,
    collection: &CollectionState,
    record: &RecordEntry,
    file: &FileEntry,
    bytes: &[u8],
    filter: Option<&BoundExpr>,
    fts: Option<&BoundFts>,
    direct_plan: Option<&DirectSourcePlan<'_>>,
    projection: &ScanProjection,
    filter_proof: Option<&[String]>,
    retain_evidence: bool,
    execution: Option<&ExecutionContext>,
) -> Result<(ExtractedRecord, bool, HashSet<String>, bool), String> {
    let _fold_record = FoldRecordGuard::new_with_sharing(shared_fold_bound(filter));
    if record.class == RecordClass::Logical
        && let Some(direct) = direct_plan
    {
        if let Some(term) = direct.original_record_matcher {
            let matched = term
                .matcher
                .as_ref()
                .ok_or("direct original-record matcher missing")?
                .matches(None, Some(bytes))?;
            let mut evidence = HashSet::new();
            if matched && retain_evidence {
                evidence.insert(term.query_path.display.clone());
            }
            return Ok((
                ExtractedRecord {
                    value: Value::Object(Default::default()),
                    text: Vec::new(),
                },
                matched,
                evidence,
                false,
            ));
        }
        let result = crate::source::verification::scan_borrowed_segments(
            collection.required_file_reader(),
            bytes,
            &BorrowedScanQuery {
                fts: direct.fts,
                filter: direct.filter.map(|(term, _)| term),
                retain_evidence,
            },
        )?;
        let matched = result.filter_match && result.fts_match;
        let mut evidence = if matched && retain_evidence {
            result.evidence
        } else {
            HashSet::new()
        };
        if matched
            && retain_evidence
            && let Some((term, _)) = direct.filter
            && term.operator == Operator::Contains
        {
            evidence.insert(term.query_path.display.clone());
        }
        return Ok((
            ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            },
            matched,
            evidence,
            false,
        ));
    }
    let borrowed_value_text = borrowed_value_text(collection, fts);
    let combined_fts = fts.filter(|fts| {
        collection.required_file_reader().scalar_verification()
            == Some(crate::source::verification::ScalarVerification::DecodedScalars)
            && source_fts_eligible(collection, fts, true)
    });
    let (extracted, combined_result) = if record.class == RecordClass::Logical
        && let Some(combined_fts) = combined_fts
    {
        let (extracted, result) = prepare_decoded_record_with_borrowed_fts(
            &collection.required_file_reader(),
            &collection.definition.schema,
            bytes,
            projection,
            combined_fts,
            retain_evidence,
            execution,
        )?;
        (extracted, Some(result))
    } else if record.class == RecordClass::Logical {
        (
            prepare_query_record_with_execution(
                record.class,
                &collection.required_file_reader(),
                &collection.definition.schema,
                bytes,
                projection,
                false,
                execution,
            )?,
            None,
        )
    } else {
        (
            ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            },
            None,
        )
    };
    let ctx = RecordContext {
        value: &extracted.value,
        source_bytes: Some(bytes),
        record,
        range: (record).record_range(),
        file: Some(file),
        collection_name: &collection.definition.name,
        repository_root: Some(repo.required_file_root()),
    };
    let mut evidence = HashSet::new();
    let filter_matches = if let Some(fields) = filter_proof {
        if retain_evidence {
            evidence.extend(fields.iter().cloned());
        }
        true
    } else if let Some(filter) = filter {
        if record.class == RecordClass::RawFallback {
            eval_unavailable_filter(filter, &ctx, &mut evidence)? == Some(true)
        } else {
            let mut work = crate::query_execution::relations::work::RelationWork::default();
            if retain_evidence {
                crate::query_execution::relations::evaluation::eval_filter_truth(
                    repo,
                    filter,
                    &ctx,
                    &mut evidence,
                    crate::query_execution::relations::evaluation::EvaluationMode::Indexed,
                    &mut work,
                )? == Some(true)
            } else {
                crate::query_execution::relations::evaluation::eval_filter_truth_membership(
                    repo,
                    filter,
                    &ctx,
                    crate::query_execution::relations::evaluation::EvaluationMode::Indexed,
                    &mut work,
                )? == Some(true)
            }
        }
    } else {
        true
    };
    if !filter_matches {
        return Ok((extracted, false, evidence, false));
    }
    let raw_checked =
        record.class == RecordClass::RawFallback && fts.is_some_and(|fts| fts.fields.is_empty());
    let fts_matches = if let Some(fts) = fts {
        if record.class == RecordClass::RawFallback && fts.fields.is_empty() {
            let matched =
                crate::reference::fts::raw_fallback_contains(fts, &String::from_utf8_lossy(bytes));
            if matched && retain_evidence {
                evidence.insert("@record.raw_text".into());
            }
            matched
        } else if let Some(result) = &combined_result {
            if retain_evidence {
                evidence.extend(result.evidence.iter().cloned());
            }
            result.fts_match
        } else if borrowed_value_text && record.class == RecordClass::Logical {
            crate::reference::fts::eval_borrowed_value_fts(
                fts,
                &ctx,
                retain_evidence.then_some(&mut evidence),
            )?
        } else {
            if retain_evidence {
                eval_bound_fts(fts, &extracted.text, &ctx, &mut evidence)?
            } else {
                crate::reference::fts::eval_bound_fts_membership(fts, &extracted.text, &ctx)?
            }
        }
    } else {
        true
    };
    Ok((extracted, fts_matches, evidence, raw_checked))
}

pub(super) fn scan_segment_file_parallel(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    published: &PublishedSegment,
    catalog: &dyn FileCatalogReader,
    filter: Option<&BoundExpr>,
    fts: Option<&BoundFts>,
    direct_plan: Option<&DirectSourcePlan<'_>>,
    projection: &ScanProjection,
    filter_proof: Option<&[String]>,
    file_id: u32,
    ranges: &[(u32, ByteRange)],
    cap: usize,
    context: Option<&ExecutionContext>,
    cache_admission: Option<&ParallelCacheAdmission<'_>>,
) -> Result<SegmentFileScan, String> {
    if let Some(context) = context {
        context.check_string()?;
        context.charge_reads(1)?;
    }
    let collection = &repo.collections[collection_index];
    let segment = published;
    let file = segment_active_file(
        segment,
        &published.active,
        catalog,
        crate::model::FileId(file_id),
    )?;
    let file = file.as_ref();
    let path = repo.required_file_root().join(&file.relative_path);
    let mut reader = RecordRangeReader::open_for_search(&path, file.revision, context)?
        .with_backend(collection.required_file_reader().reader_handle());
    let mut result = SegmentFileScan {
        bytes_read: 0,
        scanned_records: 0,
        record_parses: 0,
        residual_evaluations: 0,
        raw_text_verifications: 0,
        sort_key_evaluations: 0,
        matches: 0,
        winners: Vec::new(),
    };
    let mut local_winners = BinaryHeap::new();
    let mut cursor = 0;
    while cursor < ranges.len() {
        if let Some(context) = context {
            context.check_string()?;
        }
        let start = cursor;
        let mut batch_bytes = 0u64;
        while cursor < ranges.len() && cursor - start < 128 {
            let next = ranges[cursor].1.len();
            if cursor > start && batch_bytes.saturating_add(next) > 1024 * 1024 {
                break;
            }
            batch_bytes = batch_bytes.saturating_add(next);
            cursor += 1;
        }
        let (read, bytes_read) = reader.read_with_byte_budget(&ranges[start..cursor], context)?;
        result.bytes_read += bytes_read;
        for (slot, bytes) in read {
            if let Some(context) = context {
                context.check_string()?;
            }
            if !published.active.contains(RecordSlot(slot))? {
                return Err("inactive segment scan slot".into());
            }
            let record = super::read_file_range_record(segment, RecordSlot(slot))?;
            if record.file.0 != file_id {
                return Err("segment scan file ownership mismatch".into());
            }
            result.record_parses += u64::from(
                record.class == RecordClass::Logical
                    && (direct_plan.is_none()
                        || fts.as_ref().is_some_and(|fts| {
                            fts.fields.is_empty()
                                || fts
                                    .fields
                                    .iter()
                                    .any(|field| field.binding_name != "default")
                        })),
            );
            result.residual_evaluations += u64::from(filter.is_some() && filter_proof.is_none());
            let admission =
                cache_admission.filter(|cache| !cache.closed.load(AtomicOrdering::Relaxed));
            let lazy_evidence = query.options.evidence
                && direct_plan.is_some()
                && admission.is_none()
                && bytes.len() as u64 <= MAX_DEFERRED_EVIDENCE_RECORD_BYTES;
            let (extracted, matched, evidence, raw_checked) = verify_local_segment_record(
                repo,
                collection,
                &record,
                file,
                &bytes,
                filter,
                fts,
                direct_plan,
                projection,
                filter_proof,
                query.options.evidence && !lazy_evidence,
                context,
            )?;
            result.raw_text_verifications += u64::from(raw_checked);
            result.scanned_records += 1;
            if !matched {
                continue;
            }
            let ctx = RecordContext {
                value: &extracted.value,
                source_bytes: Some(&bytes),
                record: &record,
                range: (record).record_range(),
                file: Some(file),
                collection_name: &collection.definition.name,
                repository_root: Some(repo.required_file_root()),
            };
            result.matches += 1;
            result.sort_key_evaluations += u64::from(sort.bound.has_computed);
            let key = segment_rank_key(repo, sort, collection_index, segment, &record, Some(&ctx))?;
            if sort.bound.has_computed {
                check_execution_sort_budget(context, 0, rank_key_bytes(&key), 0)?;
            }
            let matched_fields: Vec<_> = evidence.into_iter().collect();
            if let Some(cache) = admission {
                let mut builder = cache.builder.lock().map_err(|error| error.to_string())?;
                if builder.is_accepting() {
                    builder.push(
                        &key,
                        CachedMember {
                            collection_index,
                            segment_index: Some(cache.segment_index),
                            slot,
                            matched_fields: matched_fields.clone().into(),
                        },
                    );
                    if !builder.is_accepting() {
                        cache.closed.store(true, AtomicOrdering::Relaxed);
                    }
                }
            }
            if cap == 0
                || (local_winners.len() >= cap
                    && local_winners
                        .peek()
                        .is_some_and(|worst: &SegmentLocalWinner| key >= worst.key))
            {
                continue;
            }
            let winner = SegmentLocalWinner {
                key,
                slot: RecordSlot(slot),
                matched_fields,
                source_bytes: lazy_evidence.then_some(bytes),
            };
            if local_winners.len() >= cap {
                local_winners.pop();
            }
            local_winners.push(winner);
        }
    }
    result.winners = local_winners.into_sorted_vec();
    Ok(result)
}
