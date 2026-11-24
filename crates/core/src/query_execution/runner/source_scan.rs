//! Indexed source-file scanning.

use super::*;

pub(super) fn scan_file(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    residual: Option<&Residual<'_>>,
    fts: Option<&BoundFts>,
    fts_proof: Option<&HashMap<u32, HashSet<String>>>,
    direct_plan: Option<&DirectSourcePlan<'_>>,
    target_values: Option<&crate::query_execution::relations::target_cache::TargetValueCache>,
    projection: &ScanProjection,
    retain_all_evidence: bool,
    cache_budget: Option<&CacheBudget>,
    payload_cutoff: Option<&RankKey>,
    file_id: u32,
    ranges: &[(u32, ByteRange)],
    mut source_access: Option<&mut SourceAccessSession<'_>>,
    cancellation: Option<&CancellationToken>,
    max_retained_bytes: Option<u64>,
    collect_exact_members: bool,
    execution: Option<&ExecutionContext>,
) -> Result<ScannedFile, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    let collection = &repo.collections[collection_index];
    let path = repo
        .files
        .native_path(crate::model::FileId(file_id))
        .ok_or("missing source path")?;
    let revision = repo.files.files[file_id as usize].revision;
    let mut outcome = ScannedFile::default();
    outcome.relation_work.attach(execution);
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let keep_cap = if query.options.limit == 0 { 0 } else { cap };
    let mut winners = BinaryHeap::<FileWinner>::new();
    let mut retained_bytes = 0u64;
    let mut retained_key_bytes = 0u64;
    let mut buffered_bytes = 0usize;
    let mut reader = if source_access.is_some() {
        None
    } else {
        Some(RecordRangeReader::open_for_search(
            &path, revision, execution,
        )?)
    };
    outcome.files_opened = u64::from(reader.is_some());
    // These built-in readers search the converted logical strings. Keep the
    // value, but avoid producing a second owned copy of every string and path.
    let borrowed_value_text = borrowed_value_text(collection, fts);
    let share_fold = shared_fold_residual(residual);
    let full_projection = ScanProjection::full(projection.collect_text);
    let mut cursor = 0;
    while cursor < ranges.len() {
        if let Some(execution) = execution {
            execution.check_string()?;
            if ranges[cursor].1.len() > execution.limits.source_record_bytes {
                return Err(execution.fail(
                    ExecutionErrorKind::Resource,
                    "source record exceeds read allowance",
                ));
            }
        }
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err("stream cancelled".into());
        }
        let mut end = cursor + 1;
        let mut batch_bytes = ranges[cursor].1.len();
        let batch_limit = max_retained_bytes.unwrap_or(1024 * 1024).min(1024 * 1024);
        while end < ranges.len()
            && end - cursor < 128
            && batch_bytes.saturating_add(ranges[end].1.len()) <= batch_limit
        {
            batch_bytes += ranges[end].1.len();
            end += 1;
        }
        let batch = &ranges[cursor..end];
        if let Some(execution) = execution
            && batch
                .iter()
                .any(|(_, range)| range.len() > execution.limits.source_record_bytes)
        {
            return Err(execution.fail(
                ExecutionErrorKind::Resource,
                "source record exceeds read allowance",
            ));
        }
        if max_retained_bytes
            .is_some_and(|limit| batch.iter().any(|(_, range)| range.len() > limit))
        {
            return Err(computed_sort_resource(
                execution,
                "stream resource: record exceeds buffered-byte limit",
            ));
        }
        cursor = end;
        let (records, bytes_read) = if let Some(session) = source_access.as_deref_mut() {
            let read = if let Some(limit) = max_retained_bytes {
                session.read_bounded(
                    crate::model::FileId(file_id),
                    batch,
                    usize::try_from(limit).unwrap_or(usize::MAX),
                )?
            } else {
                session.read(crate::model::FileId(file_id), batch)?
            };
            outcome.files_opened += read.opened;
            (read.records, read.bytes_read)
        } else {
            let reader = reader.as_mut().ok_or("missing source reader")?;
            if let Some(limit) = max_retained_bytes {
                reader.read_bounded(batch, usize::try_from(limit).unwrap_or(usize::MAX))?
            } else {
                reader.read(batch)?
            }
        };
        outcome.bytes_read += bytes_read;
        for (slot, bytes) in records {
            let _fold_record = FoldRecordGuard::new_with_sharing(share_fold);
            if let Some(execution) = execution {
                execution.check_string()?;
            }
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                return Err("stream cancelled".into());
            }
            let record = &collection.generation.records[slot as usize];
            let file = &repo.files.files[file_id as usize];
            let may_reach_page = !sort.bound.has_computed
                && keep_cap > 0
                && (winners.len() < keep_cap
                    || winners.peek().is_some_and(|worst| {
                        sort.compare_to_rank(
                            collection_index,
                            record,
                            &collection.definition.name,
                            &worst.key,
                        )
                        .is_lt()
                    }))
                && payload_cutoff.is_none_or(|worst| {
                    sort.compare_to_rank(
                        collection_index,
                        record,
                        &collection.definition.name,
                        worst,
                    )
                    .is_lt()
                });
            let logical_projection =
                query.options.return_mode == ReturnMode::Logical && may_reach_page;
            let direct = direct_plan.filter(|_| {
                record.class == RecordClass::Logical
                    && query.options.return_mode != ReturnMode::Logical
            });
            let record_projection = if logical_projection {
                &full_projection
            } else {
                projection
            };
            let decode = direct.is_none()
                && record.class == RecordClass::Logical
                && (logical_projection
                    || record_projection.collect_text
                    || record_projection
                        .top_level
                        .as_ref()
                        .is_none_or(|fields| !fields.is_empty()));
            let combined_fts = decode
                .then_some(())
                .filter(|_| {
                    collection.required_file_reader().scalar_verification()
                        == Some(crate::source::verification::ScalarVerification::DecodedScalars)
                })
                .and_then(|_| fts.filter(|fts| source_fts_eligible(collection, fts, true)));
            let (extracted, combined_result) = if let Some(combined_fts) = combined_fts {
                let (extracted, result) = prepare_decoded_record_with_borrowed_fts(
                    &collection.required_file_reader(),
                    &collection.definition.schema,
                    &bytes,
                    record_projection,
                    combined_fts,
                    query.options.evidence,
                    execution,
                )?;
                outcome.record_parses += 1;
                (extracted, Some(result))
            } else if decode {
                let extracted = prepare_query_record_with_execution(
                    record.class,
                    &collection.required_file_reader(),
                    &collection.definition.schema,
                    &bytes,
                    record_projection,
                    logical_projection,
                    execution,
                )?;
                outcome.record_parses += 1;
                (extracted, None)
            } else {
                (
                    ExtractedRecord {
                        value: Value::Object(Default::default()),
                        text: Vec::new(),
                    },
                    None,
                )
            };
            outcome.scanned_records += 1;
            let ctx = RecordContext {
                value: &extracted.value,
                source_bytes: Some(&bytes),
                record,
                file,
                collection_name: &collection.definition.name,
                repository_root: &repo.files.required_root(),
            };
            let mut evidence = HashSet::new();
            let direct_result = if let Some(direct) = direct {
                outcome.record_parses += 1;
                let filter = direct.filter.and_then(|(term, proof)| {
                    Residual::proven(proof, slot).is_none().then_some(term)
                });
                direct
                    .scalar_verification
                    .ok_or("direct scalar verification missing")?
                    .scan_borrowed_segments(
                        &bytes,
                        &BorrowedScanQuery {
                            fts: direct.fts,
                            filter,
                            retain_evidence: query.options.evidence,
                        },
                    )
                    .map(Some)?
            } else {
                None
            };
            let fts_match = if let Some(result) = &direct_result {
                if query.options.evidence {
                    evidence.extend(result.evidence.iter().cloned());
                }
                result.fts_match
            } else if let Some(result) = &combined_result {
                if query.options.evidence {
                    evidence.extend(result.evidence.iter().cloned());
                }
                result.fts_match
            } else if let Some(proof) = fts_proof {
                if let Some(fields) = proof.get(&slot) {
                    if query.options.evidence {
                        evidence.extend(fields.iter().cloned());
                    }
                    true
                } else {
                    false
                }
            } else if record.class == RecordClass::RawFallback {
                if let Some(fts) = fts {
                    if fts.fields.is_empty() {
                        outcome.raw_text_verifications += 1;
                        let matched = crate::reference::fts::raw_fallback_contains(
                            fts,
                            &String::from_utf8_lossy(&bytes),
                        );
                        if matched && query.options.evidence {
                            evidence.insert("@record.raw_text".into());
                        }
                        matched
                    } else {
                        if query.options.evidence {
                            crate::query_execution::relations::evaluation::eval_fts_with_relations(
                                repo,
                                fts,
                                &[],
                                &ctx,
                                &mut evidence,
                                crate::query_execution::relations::evaluation::EvaluationMode::Indexed,
                                &mut outcome.relation_work,
                            )?
                        } else {
                            crate::query_execution::relations::evaluation::eval_fts_with_relations_membership(
                                repo,
                                fts,
                                &[],
                                &ctx,
                                crate::query_execution::relations::evaluation::EvaluationMode::Indexed,
                                &mut outcome.relation_work,
                            )?
                        }
                    }
                } else {
                    true
                }
            } else if borrowed_value_text && fts.is_some() {
                crate::reference::fts::eval_borrowed_value_fts(
                    fts.expect("checked above"),
                    &ctx,
                    query.options.evidence.then_some(&mut evidence),
                )?
            } else {
                fts.map(|fts| {
                    if query.options.evidence {
                        crate::query_execution::relations::evaluation::eval_fts_with_relations(
                            repo,
                            fts,
                            &extracted.text,
                            &ctx,
                            &mut evidence,
                            crate::query_execution::relations::evaluation::EvaluationMode::Indexed,
                            &mut outcome.relation_work,
                        )
                    } else {
                        crate::query_execution::relations::evaluation::eval_fts_with_relations_membership(
                            repo,
                            fts,
                            &extracted.text,
                            &ctx,
                            crate::query_execution::relations::evaluation::EvaluationMode::Indexed,
                            &mut outcome.relation_work,
                        )
                    }
                })
                .transpose()?
                .unwrap_or(true)
            };
            let filter_match = if fts_match && let Some(result) = &direct_result {
                let direct = direct.expect("direct result has a plan");
                let matched = if let Some((_, proof)) = direct.filter {
                    if let Some(proven) = Residual::proven(proof, slot) {
                        proven
                    } else {
                        outcome.residual_evaluations += 1;
                        result.filter_match
                    }
                } else {
                    true
                };
                if matched
                    && query.options.evidence
                    && ((retain_all_evidence && cache_budget.is_some_and(CacheBudget::complete))
                        || may_reach_page
                        || sort.bound.has_computed)
                    && let Some((term, _)) = direct.filter
                    && term.operator == Operator::Contains
                {
                    evidence.insert(term.query_path.display.clone());
                }
                matched
            } else if fts_match && record.class == RecordClass::RawFallback {
                residual
                    .map(|f| f.unavailable(&ctx, &mut evidence, &mut outcome.residual_evaluations))
                    .transpose()?
                    .flatten()
                    == Some(true)
                    || residual.is_none()
            } else if fts_match {
                residual
                    .map(|f| {
                        if query.options.evidence
                            && ((retain_all_evidence
                                && cache_budget.is_some_and(CacheBudget::complete))
                                || may_reach_page
                                || sort.bound.has_computed)
                        {
                            f.with_evidence(
                                repo,
                                slot,
                                &ctx,
                                &mut evidence,
                                &mut outcome.residual_evaluations,
                                &mut outcome.relation_work,
                                target_values,
                            )
                        } else {
                            f.membership(
                                repo,
                                slot,
                                &ctx,
                                &mut outcome.residual_evaluations,
                                &mut outcome.relation_work,
                                target_values,
                            )
                        }
                    })
                    .transpose()?
                    .unwrap_or(Some(true))
                    == Some(true)
            } else {
                false
            };
            if fts_match && filter_match {
                outcome.match_count += 1;
                if collect_exact_members {
                    outcome.exact_members.insert(slot);
                }
                let evidence_owner: Arc<[String]> = if query.options.evidence {
                    let mut fields = evidence.iter().cloned().collect::<Vec<_>>();
                    fields.sort();
                    fields.into()
                } else {
                    Vec::new().into()
                };
                if let Some(budget) = cache_budget
                    && budget.reserve(evidence_owner.as_ref())
                {
                    outcome.cache_members.push(CachedMember {
                        collection_index,
                        segment_index: None,
                        slot,
                        matched_fields: Arc::clone(&evidence_owner),
                    });
                }
                let computed_key = if sort.bound.has_computed {
                    outcome.sort_key_evaluations += 1;
                    Some(rank_key_from_context(
                        sort,
                        repo,
                        collection_index,
                        &ctx,
                        target_values,
                    )?)
                } else {
                    None
                };
                let enters_page = if let Some(key) = &computed_key {
                    keep_cap > 0
                        && (winners.len() < keep_cap
                            || winners.peek().is_some_and(|worst| key < &worst.key))
                        && payload_cutoff.is_none_or(|worst| key < worst)
                } else {
                    may_reach_page
                };
                if enters_page {
                    let retained = if sort.bound.has_computed {
                        0
                    } else {
                        match query.options.return_mode {
                            ReturnMode::Locator => 0,
                            _ => bytes.len() as u64,
                        }
                    };
                    let replaced = if winners.len() == keep_cap {
                        winners
                            .peek()
                            .map_or(0, |worst| worst.matched.retained_bytes)
                    } else {
                        0
                    };
                    if max_retained_bytes.is_some_and(|limit| {
                        retained_bytes
                            .saturating_sub(replaced)
                            .saturating_add(retained)
                            > limit
                    }) {
                        return Err(computed_sort_resource(
                            execution,
                            "stream resource: file winners exceed buffered-byte limit",
                        ));
                    }
                    let payload = if sort.bound.has_computed {
                        None
                    } else if record.class == RecordClass::RawFallback
                        && query.options.return_mode == ReturnMode::Logical
                    {
                        Some(Payload::LogicalUnavailable {
                            diagnostic_ref: record.diagnostic_ref,
                        })
                    } else {
                        let payload_record = if direct.is_some()
                            && query.options.return_mode == ReturnMode::Logical
                        {
                            prepare_query_record_with_execution(
                                record.class,
                                &collection.required_file_reader(),
                                &collection.definition.schema,
                                &bytes,
                                &ScanProjection::full(false),
                                true,
                                execution,
                            )?
                        } else {
                            extracted
                        };
                        materialize(&query.options.return_mode, bytes, Some(payload_record))?
                    };
                    let key = computed_key
                        .unwrap_or_else(|| rank_key(sort, repo, collection_index, record));
                    let key_bytes = if sort.bound.has_computed || max_retained_bytes.is_some() {
                        rank_key_bytes(&key)
                    } else {
                        0
                    };
                    let measured = if sort.bound.has_computed || max_retained_bytes.is_some() {
                        std::mem::size_of::<FileWinner>()
                            .saturating_add(key_bytes as usize)
                            .saturating_add(
                                evidence.iter().map(|field| field.len() + 64).sum::<usize>(),
                            )
                            .saturating_add(
                                payload
                                    .as_ref()
                                    .map_or(0, crate::query_execution::streaming::payload_bytes),
                            )
                    } else {
                        0
                    };
                    let replaced_buffered = if (sort.bound.has_computed
                        || max_retained_bytes.is_some())
                        && winners.len() == keep_cap
                    {
                        winners
                            .peek()
                            .map_or(0, |worst| worst.matched.buffered_bytes)
                    } else {
                        0
                    };
                    if max_retained_bytes.is_some_and(|limit| {
                        buffered_bytes
                            .saturating_sub(replaced_buffered)
                            .saturating_add(measured)
                            > limit as usize
                    }) {
                        return Err(if sort.bound.has_computed && cancellation.is_none() {
                            computed_sort_resource(
                                execution,
                                "computed sort memory budget exceeded",
                            )
                        } else {
                            computed_sort_resource(
                                execution,
                                "stream resource: file winners exceed buffered-byte limit",
                            )
                        });
                    }
                    if winners.len() == keep_cap {
                        let removed = winners.pop().unwrap();
                        retained_bytes -= removed.matched.retained_bytes;
                        if sort.bound.has_computed {
                            retained_key_bytes -= rank_key_bytes(&removed.key);
                        }
                        buffered_bytes -= removed.matched.buffered_bytes;
                    }
                    retained_bytes += retained;
                    buffered_bytes += measured;
                    outcome.peak_payload_bytes = outcome.peak_payload_bytes.max(retained_bytes);
                    if sort.bound.has_computed {
                        retained_key_bytes = retained_key_bytes
                            .checked_add(key_bytes)
                            .ok_or("sort key byte count overflow")?;
                        if retained_key_bytes > MAX_SORT_KEY_BYTES {
                            return Err(computed_sort_resource(
                                execution,
                                "sort key memory budget exceeded",
                            ));
                        }
                        check_execution_sort_budget(
                            execution,
                            retained_bytes,
                            retained_key_bytes,
                            0,
                        )?;
                        outcome.peak_sort_key_bytes =
                            outcome.peak_sort_key_bytes.max(retained_key_bytes);
                    }
                    winners.push(FileWinner {
                        key,
                        matched: ScannedMatch {
                            slot,
                            key: None,
                            evidence: evidence_owner,
                            payload,
                            retained_bytes: retained,
                            buffered_bytes: measured,
                        },
                    });
                }
            }
        }
    }
    outcome.matches = winners
        .into_sorted_vec()
        .into_iter()
        .map(|mut winner| {
            winner.matched.key = Some(winner.key);
            winner.matched
        })
        .collect();
    Ok(outcome)
}
