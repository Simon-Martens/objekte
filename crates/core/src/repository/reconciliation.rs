//! Source manifest reconciliation and dirty-state recovery.

use super::*;

impl Repository {
    pub(super) fn sync(
        &mut self,
        c: &Collection,
        s: &Store,
        requested_indexes: &[String],
        force: bool,
    ) -> Result<()> {
        let before = self.metrics.clone();
        let t = Instant::now();
        let result = self.sync_inner(c, s, requested_indexes, force);
        self.metrics.sync_nanos += nanos(t);
        if result.is_err() {
            self.verified.remove(&c.name);
            let _ = s.dirty(&c.name);
            self.metrics.rollback_writes(&before);
        }
        result
    }

    pub(super) fn sync_inner(
        &mut self,
        c: &Collection,
        s: &Store,
        requested_indexes: &[String],
        force: bool,
    ) -> Result<()> {
        let table = store::record_table(&c.name);
        let previous = s.state(&c.name)?;
        let has_indexes = s.has_indexes(c)?;
        let requested_ready = s.has_requested_indexes(requested_indexes)?;
        let on_open = matches!(
            self.options.freshness,
            Freshness::MetadataOnOpen | Freshness::HashOnOpen
        );
        if !force
            && on_open
            && previous.as_ref().is_some_and(|p| {
                p.state == "current"
                    && p.schema == c.schema_hash
                    && p.mapping == c.mapping_hash
                    && (!has_indexes || p.index == c.index_hash)
                    && p.view == c.view_hashes.combined
                    && self.verified.get(&c.name) == Some(&p.revision)
            })
        {
            if !requested_ready {
                let t = Instant::now();
                let tx = s.db.unchecked_transaction()?;
                let _build = s.index_cache.build(c, requested_indexes, false)?;
                if requested_indexes.len() > 1 {
                    s.index_cache.freeze()?;
                }
                store::create_indexes(&tx, c, requested_indexes, &mut self.metrics)?;
                store::analyze_indexes(&tx, c, requested_indexes)?;
                tx.execute(
                    "UPDATE collections SET index_hash=?2 WHERE name=?1",
                    params![c.name, c.index_hash],
                )?;
                tx.commit()?;
                self.metrics.index_nanos += nanos(t);
            }
            return Ok(());
        }
        self.verified.remove(&c.name);
        let (source, files) = self.inventory(c)?;
        let t = Instant::now();
        let manifest_hash = fingerprint("source-manifest/v2", &files);
        self.metrics.fingerprint_nanos += nanos(t);
        let metadata_mode = matches!(
            self.options.freshness,
            Freshness::MetadataOnOpen | Freshness::MetadataOnRequest
        );
        let mapping_changed = previous
            .as_ref()
            .is_some_and(|p| !p.mapping.is_empty() && p.mapping != c.mapping_hash);
        let schema_changed =
            previous.as_ref().is_none_or(|p| p.schema != c.schema_hash) || mapping_changed;
        let use_indexes = !requested_indexes.is_empty() || has_indexes;
        let rebuild = use_indexes
            && (!has_indexes
                || mapping_changed
                || previous.as_ref().is_none_or(|p| p.index != c.index_hash));
        let build_indexes = !requested_indexes.is_empty() && (rebuild || !requested_ready);
        let complete = previous.as_ref().is_some_and(|p| p.state == "current");
        if metadata_mode
            && complete
            && !schema_changed
            && !rebuild
            && !build_indexes
            && previous
                .as_ref()
                .is_some_and(|p| p.manifest == manifest_hash && p.records == files.len())
        {
            // PERFORMANCE: Unchanged metadata avoids the per-record DB manifest and source bytes.
            let previous = previous.as_ref().unwrap();
            source.finish()?;
            if previous.view != c.view_hashes.combined {
                s.db.execute(
                    "UPDATE collections SET view_hash=?2 WHERE name=?1",
                    params![c.name, c.view_hashes.combined],
                )?;
            }
            self.metrics.manifest_hits += 1;
            self.metrics.metadata_reused += files.len() as u64;
            self.metrics.rows_reused += files.len() as u64;
            self.metrics.reconciliations += 1;
            self.verified.insert(c.name.clone(), previous.revision);
            return Ok(());
        }
        s.dirty(&c.name)?;
        let t = Instant::now();
        let manifest = s.manifest(&c.name)?;
        self.metrics.manifest_nanos += nanos(t);
        self.metrics.manifest_loads += 1;
        let before = self.metrics.clone();
        let tx = s.db.unchecked_transaction()?;
        if mapping_changed {
            for id in manifest.keys() {
                store::delete_record(&tx, c, id, &mut self.metrics)?;
            }
        }
        if rebuild {
            let t = Instant::now();
            store::drop_indexes(&tx, c)?;
            self.metrics.index_nanos += nanos(t);
        }
        let incremental = build_indexes && manifest.is_empty();
        if incremental {
            let t = Instant::now();
            store::create_indexes(&tx, c, requested_indexes, &mut self.metrics)?;
            self.metrics.index_nanos += nanos(t);
        }
        let _build = if build_indexes {
            Some(s.index_cache.build(c, requested_indexes, incremental)?)
        } else {
            None
        };
        let indexed = (use_indexes && (!rebuild || build_indexes)).then_some(&s.index_cache);
        if manifest.is_empty() && self.options.ingest_workers > 1 && files.len() > 1 {
            self.ingest_empty(&tx, c, &source, &files, indexed)?;
        } else {
            for (id, stamp) in &files {
                let old = if mapping_changed {
                    None
                } else {
                    manifest.get(id)
                };
                let candidate =
                    !metadata_mode || !complete || old.is_none_or(|v| &v.stamp != stamp);
                if candidate {
                    let raw = self.raw_from(&source, id, Some(stamp))?;
                    let changed = old.is_none_or(|v| v.digest != raw.hash);
                    if changed || schema_changed {
                        let value = self.parse(c, id, &raw.json, false);
                        self.put_record(&tx, c, id, &raw, &value, indexed)?;
                    } else {
                        self.metrics.rows_reused += 1;
                        if old.is_some_and(|v| v.stamp != raw.stamp) {
                            tx.prepare_cached(&format!("UPDATE {table} SET size=?3,modified=?4,identity=?5 WHERE collection=?1 AND id=?2"))?
                            .execute(params![c.name,id,raw.stamp.size as i64,raw.stamp.modified,raw.stamp.identity])?;
                        }
                    }
                } else {
                    self.metrics.metadata_reused += 1;
                    if schema_changed {
                        // INFO: Cached bytes correspond to the unchanged manifest entry. Schema
                        // changes need revalidation, not another source-file read.
                        let json: Vec<u8> = tx.query_row(
                            &format!("SELECT json FROM {table} WHERE collection=?1 AND id=?2"),
                            params![c.name, id],
                            |r| Ok(r.get_ref(0)?.as_bytes()?.to_vec()),
                        )?;
                        let value = self.parse(c, id, &json, false);
                        let raw = Raw {
                            json,
                            hash: old.unwrap().digest.clone(),
                            stamp: stamp.clone(),
                        };
                        self.put_record(&tx, c, id, &raw, &value, indexed)?;
                    } else {
                        self.metrics.rows_reused += 1;
                    }
                }
            }
        }
        if !mapping_changed {
            for id in manifest.keys().filter(|id| !files.contains_key(*id)) {
                store::delete_record(&tx, c, id, &mut self.metrics)?;
            }
        }
        source.finish()?;
        // INFO: Strict mode retains a final all-file stability check. Metadata mode enumerates
        // once, with pinned-directory and before/after descriptor checks for read candidates.
        if !metadata_mode && self.inventory(c)?.1 != files {
            return Err(Error::Unstable(c.path.clone()));
        }
        if build_indexes {
            let t = Instant::now();
            if !incremental {
                if requested_indexes.len() > 1 {
                    s.index_cache.freeze()?;
                }
                store::create_indexes(&tx, c, requested_indexes, &mut self.metrics)?;
            }
            store::analyze_indexes(&tx, c, requested_indexes)?;
            self.metrics.index_nanos += nanos(t);
        }
        let changed = schema_changed
            || rebuild
            || self.metrics.rows_inserted != before.rows_inserted
            || self.metrics.rows_updated != before.rows_updated
            || self.metrics.rows_deleted != before.rows_deleted;
        let revision = previous.as_ref().map_or(0, |p| p.revision) + i64::from(changed);
        let t = Instant::now();
        tx.execute("UPDATE collections SET state='current',schema_hash=?2,mapping_hash=?3,index_hash=?4,revision=?5,manifest_hash=?6,record_count=?7,view_hash=?8 WHERE name=?1",params![c.name,c.schema_hash,c.mapping_hash,if use_indexes{c.index_hash.as_str()}else{""},revision,manifest_hash,files.len() as i64,c.view_hashes.combined])?;
        tx.commit()?;
        self.metrics.commit_nanos += nanos(t);
        self.verified.insert(c.name.clone(), revision);
        self.metrics.reconciliations += 1;
        Ok(())
    }

    pub fn refresh(&mut self, collection: &str) -> Result<Metrics> {
        self.reload()?;
        let before = self.metrics.clone();
        let c = self.collection(collection)?;
        if self.options.cache == CacheMode::Sqlite {
            let writer = self.store()?;
            let s = writer.enter(self.options.sqlite_cache_mib, self.options.sql_diagnostics)?;
            self.sync(&c, &s, &[], true)?;
        } else {
            self.direct(&c, None)?;
        }
        self.schedule_validation(&c)?;
        Ok(self.metrics.since(&before))
    }
}
