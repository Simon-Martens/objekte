//! Parallel source preparation followed by ordered SQLite writes.

use super::*;

impl Repository {
    pub(super) fn ingest_empty(
        &mut self,
        db: &rusqlite::Connection,
        c: &Collection,
        source: &Source,
        files: &BTreeMap<String, Stamp>,
        indexed: Option<&crate::json_filter::indexes::IndexCache>,
    ) -> Result<()> {
        let t = Instant::now();
        let pool = rayon::ThreadPoolBuilder::new()
            // INFO: Reserve sufficient virtual stack for Sonic's bounded nesting in debug builds.
            .stack_size(16 * 1024 * 1024)
            .num_threads(self.options.ingest_workers.min(files.len()))
            .build()
            .map_err(|e| Error::Cache(format!("create ingestion workers: {e}")))?;
        self.metrics.parallel_prepare_nanos += nanos(t);
        let mut remaining = files.iter().peekable();
        while remaining.peek().is_some() {
            // PERFORMANCE: Bound retained documents by both count and source-byte size. One
            // oversized record is processed alone; this is not a total RSS limit.
            let batch = next_ingest_batch(&mut remaining);
            let t = Instant::now();
            let prepared = pool.install(|| {
                batch
                    .par_iter()
                    .map(|(id, stamp)| {
                        let mut metrics = Metrics::default();
                        let result = (|| {
                            let raw = Self::read_raw(&mut metrics, source, id, Some(stamp))?;
                            let value = Self::parse_value(&mut metrics, c, id, &raw.json, false);
                            Ok((raw, value))
                        })();
                        (metrics, result)
                    })
                    .collect::<Vec<(Metrics, Result<(Raw, Document)>)>>()
            });
            self.metrics.parallel_prepare_nanos += nanos(t);
            // INFO: Account for all worker attempts, including those after the first
            // ordered error. Only this thread touches SQLite, in manifest order.
            for (metrics, _) in &prepared {
                self.metrics.merge(metrics);
            }
            for ((id, _), (_, result)) in batch.into_iter().zip(prepared) {
                let (raw, value) = result?;
                self.put_record(db, c, id, &raw, &value, indexed)?;
            }
        }
        Ok(())
    }

    pub(super) fn put_record(
        &mut self,
        db: &rusqlite::Connection,
        c: &Collection,
        id: &str,
        raw: &Raw,
        value: &Document,
        indexed: Option<&crate::json_filter::indexes::IndexCache>,
    ) -> Result<()> {
        let t = Instant::now();
        let table = store::record_table(&c.name);
        let exists: Option<String> = db
            .prepare_cached(&format!(
                "SELECT digest FROM {table} WHERE collection=?1 AND id=?2"
            ))?
            .query_row(params![c.name, id], |r| r.get(0))
            .optional()?;
        if exists.as_ref() != Some(&raw.hash) {
            let maintained = if let Some(index_cache) = indexed {
                index_cache.prime(
                    &raw.json,
                    (value.state.status != RecordStatus::IncorrectJson).then_some(&value.data),
                )?
            } else {
                false
            };
            db.prepare_cached(&format!("INSERT INTO {table}(collection,id,path,size,modified,identity,digest,json,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1) ON CONFLICT(collection,id) DO UPDATE SET path=excluded.path,size=excluded.size,modified=excluded.modified,identity=excluded.identity,digest=excluded.digest,json=excluded.json,revision={table}.revision+1"))?.execute(params![c.name,id,self.path(c,id),raw.stamp.size as i64,raw.stamp.modified,raw.stamp.identity,raw.hash,raw.json])?;
            db.prepare_cached("INSERT INTO record_validation(collection,id,parse_status,validation_state,validation_schema_hash,issues) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(collection,id) DO UPDATE SET parse_status=excluded.parse_status,validation_state=excluded.validation_state,validation_schema_hash=excluded.validation_schema_hash,issues=excluded.issues")?.execute(params![c.name,id,if value.state.status == RecordStatus::IncorrectJson {"incorrect_json"} else {"parsed"},value.state.status.text(),c.schema_hash,serde_json::to_string(&value.state.issues).unwrap()])?;
            if maintained {
                self.metrics.index_rows_written += 1;
            }
            if exists.is_some() {
                self.metrics.rows_updated += 1
            } else {
                self.metrics.rows_inserted += 1
            }
        } else {
            db.prepare_cached(&format!(
                "UPDATE {table} SET size=?3,modified=?4,identity=?5 WHERE collection=?1 AND id=?2"
            ))?
            .execute(params![
                c.name,
                id,
                raw.stamp.size as i64,
                raw.stamp.modified,
                raw.stamp.identity
            ])?;
            // FIX: A schema change invalidates diagnostics even if source bytes did not change.
            db.execute("UPDATE record_validation SET validation_state=?3,validation_schema_hash=?4,issues=?5 WHERE collection=?1 AND id=?2 AND validation_schema_hash<>?4", params![c.name,id,value.state.status.text(),c.schema_hash,serde_json::to_string(&value.state.issues).unwrap()])?;
            self.metrics.rows_reused += 1;
        }
        self.metrics.record_write_nanos += nanos(t);
        let t = Instant::now();
        let edges = self.extract(c, id, value)?;
        self.metrics.relation_extract_nanos += nanos(t);
        store::put_edges(db, &c.name, id, &edges, &mut self.metrics)?;
        Ok(())
    }
}

const MAX_INGEST_BATCH_RECORDS: usize = 256;
const MAX_INGEST_BATCH_BYTES: u64 = 8 * 1024 * 1024;

fn next_ingest_batch<'a, I>(remaining: &mut std::iter::Peekable<I>) -> Vec<(&'a String, &'a Stamp)>
where
    I: Iterator<Item = (&'a String, &'a Stamp)>,
{
    let mut batch = Vec::new();
    let mut bytes = 0_u64;
    while let Some((_, stamp)) = remaining.peek() {
        if !batch.is_empty()
            && (batch.len() >= MAX_INGEST_BATCH_RECORDS
                || bytes.saturating_add(stamp.size) > MAX_INGEST_BATCH_BYTES)
        {
            break;
        }
        bytes = bytes.saturating_add(stamp.size);
        batch.push(remaining.next().unwrap());
    }
    batch
}

#[cfg(test)]
#[path = "ingestion.tests.rs"]
mod batch_tests;
