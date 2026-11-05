//! Authoritative source inventory, descriptor reads, and JSON parsing.

use super::*;


pub(super) struct VerifiedReadMetadata {
    pub stamp: Stamp,
    pub created: Option<i128>,
}


impl Repository {
    pub(super) fn collection(&self, name: &str) -> Result<Collection> {
        Ok(self.model.collection(name)?.clone())
    }

    /// Absolute source directories, for sort projections of `@` fields.
    pub(super) fn collection_paths(&self, names: &[String]) -> BTreeMap<String, PathBuf> {
        names
            .iter()
            .filter_map(|name| {
                let collection = self.model.collections.get(name)?;
                Some((name.clone(), collection.path.clone()))
            })
            .collect()
    }

    pub(super) fn inventory(
        &mut self,
        c: &Collection,
    ) -> Result<(Source, BTreeMap<String, Stamp>)> {
        let t = Instant::now();
        let source = Source::open_scoped(&c.path, c.recursive)?;
        self.metrics.enumerations += 1;
        let files = source.inventory()?;
        self.metrics.inventory_nanos += nanos(t);
        Ok((source, files))
    }

    pub(super) fn check_source(&self, c: &Collection) -> Result<()> {
        let actual = fs::canonicalize(&c.path).map_err(|e| io(&c.path, e))?;
        if actual != c.path || !actual.starts_with(&self.model.root) {
            return Err(record(
                &c.path,
                "",
                "source directory identity changed; reload repository",
            ));
        }
        Ok(())
    }

    pub(super) fn raw(&mut self, c: &Collection, id: &str) -> Result<Raw> {
        let t = Instant::now();
        let source = Source::open_scoped(&c.path, c.recursive)?;
        self.metrics.source_check_nanos += nanos(t);
        let raw = self.raw_from(&source, id, None)?;
        source.finish()?;
        Ok(raw)
    }

    pub(super) fn raw_from(
        &mut self,
        source: &Source,
        id: &str,
        expected: Option<&Stamp>,
    ) -> Result<Raw> {
        Self::read_raw(&mut self.metrics, source, id, expected)
    }

    pub(super) fn read_raw(
        metrics: &mut Metrics,
        source: &Source,
        id: &str,
        expected: Option<&Stamp>,
    ) -> Result<Raw> {
        let mut json = Vec::new();
        let metadata = Self::read_verified_into(metrics, source, id, expected, &mut json)?;
        let t = Instant::now();
        let hash = digest(&json);
        metrics.hash_nanos += nanos(t);
        metrics.hashes += 1;
        Ok(Raw {
            json,
            hash,
            stamp: metadata.stamp,
            created: metadata.created,
        })
    }

    /// Read through the pinned source directory, retrying an unstable descriptor.
    /// The caller owns the reusable buffer; unsuccessful candidates need no hash.
    pub(super) fn read_verified_into(
        metrics: &mut Metrics,
        source: &Source,
        id: &str,
        expected: Option<&Stamp>,
        bytes: &mut Vec<u8>,
    ) -> Result<VerifiedReadMetadata> {
        let path = source.path.join(format!("{id}.json"));
        // INFO: A source file may change during a read. Compare descriptor stamps
        // before and after, then retry rather than caching an inconsistent snapshot.
        for _ in 0..3 {
            let t = Instant::now();
            let mut file = source.file(id)?;
            let before = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
            if expected.is_some_and(|s| s != &before) {
                return Err(Error::Unstable(path));
            }
            metrics.source_check_nanos += nanos(t);
            let t = Instant::now();
            bytes.clear();
            file.read_to_end(bytes).map_err(|e| io(&path, e))?;
            metrics.read_nanos += nanos(t);
            metrics.source_reads += 1;
            metrics.source_bytes += bytes.len() as u64;
            let t = Instant::now();
            let after_metadata = file.metadata().map_err(|e| io(&path, e))?;
            let after = Stamp::from_metadata(&after_metadata, &path)?;
            metrics.source_check_nanos += nanos(t);
            if before != after || bytes.len() as u64 != after.size {
                continue;
            }
            return Ok(VerifiedReadMetadata {
                stamp: after,
                created: crate::source::created_timestamp(&after_metadata),
            });
        }
        Err(Error::Unstable(path))
    }

    pub(super) fn parse(
        &mut self,
        c: &Collection,
        id: &str,
        json: &[u8],
        validate: bool,
    ) -> Document {
        Self::parse_value(&mut self.metrics, c, id, json, validate)
    }

    pub(super) fn parse_value(
        metrics: &mut Metrics,
        c: &Collection,
        _id: &str,
        json: &[u8],
        validate: bool,
    ) -> Document {
        let t = Instant::now();
        metrics.parses += 1;
        let mut doc = Document::parse(json);
        metrics.json_decode_nanos += nanos(t);
        if c.schema.is_none() && !doc.state.status.is_malformed() {
            doc.state.status = RecordStatus::NoSchema;
        }
        if validate && c.schema.is_some() && !doc.state.status.is_malformed() {
            let t = Instant::now();
            doc.validate(c);
            metrics.validations += 1;
            metrics.validation_nanos += nanos(t);
        }
        metrics.parse_nanos += nanos(t);
        doc
    }
}
