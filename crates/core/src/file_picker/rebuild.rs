//! Rebuild immutable postings and secondary indexes from source files.

use super::*;


impl FilePicker {
    /// Rebuilds the immutable index after many edits. Source stamps are checked
    /// before and after every read so a racing edit cannot publish a bad index.
    pub fn rebuild_index(&mut self) -> Result<()> {
        let source = Source::open_scoped(&self.root, self.recursive)?;
        self.rebuild_index_from_source(&source, None)?;
        self.modified_order.rebuild(&self.files, &self.id_bytes);
        Ok(())
    }

    pub(super) fn rebuild_index_from_source(
        &mut self,
        source: &Source,
        restored: Option<RestoredBigram>,
    ) -> Result<()> {
        // A fresh picker already has compact ID-ordered slots. Later edits can
        // append slots or leave holes, in which case rebuild must compact them.
        #[cfg(feature = "index-timings")]
        let slots_started = Instant::now();
        let already_compact = self.sorted.len() == self.files.len()
            && self
                .sorted
                .iter()
                .enumerate()
                .all(|(position, &slot)| position == slot);
        let compacted = if already_compact {
            None
        } else {
            let mut ids = Vec::with_capacity(self.id_bytes.len());
            let mut files = Vec::with_capacity(self.sorted.len());
            for &slot in &self.sorted {
                let mut entry = self.files[slot].as_ref().expect("live sorted slot").clone();
                let id = self.id(slot).expect("validated ID");
                entry.id_start = u32::try_from(ids.len())
                    .map_err(|_| Error::Cache("file picker ID arena exceeds 4 GiB".into()))?;
                ids.extend_from_slice(id.as_bytes());
                files.push(Some(entry));
            }
            Some((ids, files))
        };
        let (ids, files) = compacted.as_ref().map_or(
            (self.id_bytes.as_slice(), self.files.as_slice()),
            |(ids, files)| (ids.as_slice(), files.as_slice()),
        );
        // Workers project source files once. Their local rows are merged into
        // the immutable filter and each secondary index after the scan.
        let main = BigramIndexBuilder::new_with_encoding(files.len(), self.encoding);
        let skip = BigramIndexBuilder::new_with_encoding(files.len(), self.encoding);
        // Once full-text postings are active, rebuild them with the normal
        // postings so slot compaction never requires a second source pass.
        let text_builders = (self.eager_text_index || self.text_index.is_some())
            .then(|| TextIndexBuilders::new(files.len(), self.encoding, &self.text_paths));
        let array_fields = std::sync::Mutex::new(BTreeSet::new());
        let key_summary = std::sync::Mutex::new(KeySummary::default());
        let malformed = std::sync::Mutex::new(BTreeSet::new());
        let overlay_bigrams = std::sync::Mutex::new(BTreeMap::new());
        let indexed_fields = self.typed.fields().to_vec();
        let typed_rows = std::sync::Mutex::new(vec![Vec::new(); indexed_fields.len()]);
        let posting_paths = self
            .advanced_paths
            .iter()
            .cloned()
            .chain(self.typed.dynamic_paths())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let posting_builders = posting_paths
            .iter()
            .map(|_| std::sync::Mutex::new(PostingRows::new(files.len())))
            .collect::<Vec<_>>();
        let presence_fields = self.presence.fields();
        let presence_words = self.presence.build_words(files.len());
        let view_width = self.view.field_count();
        let mut view_columns = vec![vec![crate::query::Scalar::Missing; files.len()]; view_width];
        let sort_width = self.sort.key_width();
        let mut sort_keys = vec![crate::sorting::SortValue::Missing; files.len() * sort_width];
        let chunk_count = files.len().div_ceil(64);
        let sort_slices: Vec<Option<&mut [crate::sorting::SortValue]>> = if sort_width == 0 {
            (0..chunk_count).map(|_| None).collect()
        } else {
            sort_keys.chunks_mut(64 * sort_width).map(Some).collect()
        };
        let mut view_slices = (0..chunk_count)
            .map(|_| Vec::<&mut [crate::query::Scalar]>::with_capacity(view_width))
            .collect::<Vec<_>>();
        for column in &mut view_columns {
            for (chunk, slice) in column.chunks_mut(64).enumerate() {
                view_slices[chunk].push(slice);
            }
        }
        let view = &self.view;
        let sort = &self.sort;
        #[cfg(feature = "index-timings")]
        {
            self.build_timings.slots_nanos += elapsed_nanos(slots_started);
        }

        #[cfg(feature = "index-timings")]
        let project_worker = AtomicU64::new(0);
        #[cfg(feature = "index-timings")]
        let add_worker = AtomicU64::new(0);
        #[cfg(feature = "index-timings")]
        let scan_started = Instant::now();
        scan_pool::run(|| {
            files
                .par_chunks(64)
                .zip(sort_slices.into_par_iter())
                .zip(view_slices.into_par_iter())
                .enumerate()
                .try_for_each(
                    |(chunk_index, ((chunk, mut sort_chunk), mut view_chunk))| -> Result<()> {
                        let mut chunk_arrays = BTreeSet::new();
                        let mut chunk_keys = KeySummary::default();
                        let mut chunk_malformed = BTreeSet::new();
                        let mut chunk_typed = vec![Vec::new(); indexed_fields.len()];
                        let mut chunk_postings = posting_paths
                            .iter()
                            .map(|_| ChunkRows::new())
                            .collect::<Vec<_>>();
                        let mut read_buffer = Vec::new();
                        #[cfg(feature = "index-timings")]
                        let mut chunk_project_nanos = 0;
                        #[cfg(feature = "index-timings")]
                        let mut chunk_add_nanos = 0;
                        for (within, entry) in chunk.iter().enumerate() {
                            let Some(entry) = entry else { continue };
                            let id = std::str::from_utf8(
                                &ids[entry.id_start as usize
                                    ..(entry.id_start + entry.id_len) as usize],
                            )
                            .expect("validated UTF-8 ID");
                            #[cfg(feature = "index-timings")]
                            let project_started = Instant::now();
                            let sort_output = sort_chunk.as_deref_mut().map(|keys| {
                                &mut keys[within * sort_width..(within + 1) * sort_width]
                            });
                            let projection = projected_values_with_text_buffer_and_sort(
                                source,
                                id,
                                &entry.stamp,
                                &indexed_fields,
                                presence_fields,
                                view,
                                sort,
                                ProjectionOptions {
                                    folded_index: true,
                                    text_rules: text_builders.as_ref().map(|_| &self.text_rules),
                                    text_paths: &self.text_paths,
                                    key_summary: Some(&mut chunk_keys),
                                },
                                &mut read_buffer,
                                sort_output,
                                (view_width > 0).then_some((view_chunk.as_mut_slice(), within)),
                                (!posting_paths.is_empty()).then_some((
                                    posting_paths.as_slice(),
                                    chunk_postings.as_mut_slice(),
                                    chunk_index * 64 + within,
                                )),
                            )?;
                            #[cfg(feature = "index-timings")]
                            {
                                chunk_project_nanos += elapsed_nanos(project_started);
                            }
                            let slot = chunk_index * 64 + within;
                            #[cfg(feature = "index-timings")]
                            let add_started = Instant::now();
                            match projection {
                                ProjectionOutcome::Malformed(mut text) => {
                                    chunk_malformed.insert(slot);
                                    if let Some(builders) = &text_builders {
                                        builders.add(
                                            slot,
                                            &project_malformed_text(&text, self.text_paths.len()),
                                        );
                                    }
                                    if !text.is_ascii() {
                                        let folded = text.case_fold().collect::<String>();
                                        text.push('\u{1f}');
                                        text.push_str(&folded);
                                    }
                                    if restored.is_none() {
                                        main.add_file_content(&skip, slot, text.as_bytes());
                                    } else if restored.as_ref().is_some_and(|restored| {
                                        restored.newly_changed.contains(&slot)
                                    }) {
                                        let mut keys = extract_bigrams_with_encoding(
                                            text.as_bytes(),
                                            self.encoding,
                                        );
                                        keys.sort_unstable();
                                        overlay_bigrams
                                            .lock()
                                            .expect("overlay bigram lock")
                                            .insert(slot, keys);
                                    }
                                }
                                ProjectionOutcome::Valid(projection) => {
                                    if restored.is_none() {
                                        main.add_file_content(&skip, slot, &projection.bytes);
                                    } else if restored.as_ref().is_some_and(|restored| {
                                        restored.newly_changed.contains(&slot)
                                    }) {
                                        let mut keys = extract_bigrams_with_encoding(
                                            &projection.bytes,
                                            self.encoding,
                                        );
                                        keys.sort_unstable();
                                        overlay_bigrams
                                            .lock()
                                            .expect("overlay bigram lock")
                                            .insert(slot, keys);
                                    }
                                    if let (Some(builders), Some(text)) =
                                        (&text_builders, &projection.text)
                                    {
                                        builders.add(slot, text);
                                    }

                                    chunk_arrays.extend(projection.array_fields);
                                    PresenceIndex::add_build_row(
                                        &presence_words,
                                        slot,
                                        &projection.present_fields,
                                    );
                                    for (field, key) in projection.typed_values {
                                        chunk_typed[field].push((key, slot));
                                    }
                                }
                            }

                            #[cfg(feature = "index-timings")]
                            {
                                chunk_add_nanos += elapsed_nanos(add_started);
                            }
                        }
                        array_fields
                            .lock()
                            .expect("array field lock")
                            .extend(chunk_arrays);
                        key_summary
                            .lock()
                            .expect("key summary lock")
                            .merge(chunk_keys);
                        malformed
                            .lock()
                            .expect("malformed lock")
                            .extend(chunk_malformed);
                        #[cfg(feature = "index-timings")]
                        {
                            project_worker.fetch_add(chunk_project_nanos, Ordering::Relaxed);
                        }

                        #[cfg(feature = "index-timings")]
                        {
                            add_worker.fetch_add(chunk_add_nanos, Ordering::Relaxed);
                        }
                        let mut rows = typed_rows.lock().expect("typed row lock");
                        for (all, chunk) in rows.iter_mut().zip(chunk_typed) {
                            all.extend(chunk);
                        }
                        merge_chunk(&posting_builders, chunk_postings);
                        Ok(())
                    },
                )
        })?;
        #[cfg(feature = "index-timings")]
        {
            self.build_timings.scan_nanos = elapsed_nanos(scan_started);
        }

        #[cfg(feature = "index-timings")]
        {
            self.build_timings.project_worker_nanos = project_worker.load(Ordering::Relaxed);
        }

        #[cfg(feature = "index-timings")]
        {
            self.build_timings.add_worker_nanos = add_worker.load(Ordering::Relaxed);
        }

        #[cfg(feature = "index-timings")]
        let compress_started = Instant::now();
        let (index, changed, mut changed_bigrams) = if let Some(restored) = restored {
            (restored.filter, restored.changed, restored.changed_bigrams)
        } else {
            let mut index = main.compress(None);
            index.set_skip_index(skip.compress(Some(12))); // FFF's current skip-index cutoff.
            (index, BTreeSet::new(), BTreeMap::new())
        };
        changed_bigrams.extend(overlay_bigrams.into_inner().expect("overlay bigram lock"));
        #[cfg(feature = "index-timings")]
        {
            self.build_timings.compress_nanos = elapsed_nanos(compress_started);
        }

        #[cfg(feature = "index-timings")]
        let typed_started = Instant::now();
        let mut typed = TypedIndex::new(indexed_fields);
        typed.replace_rows(typed_rows.into_inner().expect("typed row lock"));
        for (path, builder) in posting_paths.into_iter().zip(posting_builders) {
            let builder = builder.into_inner().expect("posting row lock");
            typed.replace_path(
                path,
                builder.rows,
                builder.present,
                builder.scalar_safe,
                builder.complete_values,
            );
        }

        #[cfg(feature = "index-timings")]
        {
            self.build_timings.typed_nanos = elapsed_nanos(typed_started);
        }

        #[cfg(feature = "index-timings")]
        let view_started = Instant::now();
        let view_order = self.view.ordered_columns(&view_columns, files, ids);
        let sort_order = self.sort.ordered_flat(&sort_keys, files, ids);
        #[cfg(feature = "index-timings")]
        {
            self.build_timings.view_nanos = elapsed_nanos(view_started);
        }
        let text_index = text_builders.map(TextIndexBuilders::finish);
        let malformed = malformed.into_inner().expect("malformed lock");
        let array_fields = array_fields.into_inner().expect("array field lock");
        let key_summary = key_summary.into_inner().expect("key summary lock");

        // All fallible work and sorting is complete before replacing live slots.
        source.finish()?;
        if let Some((ids, files)) = compacted {
            self.id_bytes = ids;
            self.files = files;
            self.sorted = (0..self.files.len()).collect();
        }
        self.index = index;
        self.indexed_len = self.files.len();
        self.changed = changed;
        self.changed_bigrams = changed_bigrams;
        self.malformed = malformed;
        self.text_index = text_index;
        self.array_fields = array_fields;
        self.key_summary = key_summary;
        self.presence.replace(presence_words);
        self.typed = typed;
        self.view.apply_columns(view_columns, view_order);
        self.sort.apply_flat(sort_keys, sort_order);
        Ok(())
    }
}
