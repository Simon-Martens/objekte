//! Authoritative source inventory, descriptor reads, and JSON parsing.

use super::*;


impl Repository {
    pub(super) fn collection(&self, name: &str) -> Result<Collection> {
        Ok(self.model.collection(name)?.clone())
    }

    pub(super) fn inventory(
        &mut self,
        c: &Collection,
    ) -> Result<(Source, BTreeMap<String, Stamp>)> {
        let t = Instant::now();
        let source = Source::open(&c.path)?;
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
        let source = Source::open(&c.path)?;
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
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| io(&path, e))?;
            metrics.read_nanos += nanos(t);
            metrics.source_reads += 1;
            metrics.source_bytes += bytes.len() as u64;
            let t = Instant::now();
            let after = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
            metrics.source_check_nanos += nanos(t);
            if before != after || bytes.len() as u64 != after.size {
                continue;
            }
            let t = Instant::now();
            let hash = digest(&bytes);
            metrics.hash_nanos += nanos(t);
            metrics.hashes += 1;
            let json = bytes;
            return Ok(Raw {
                json,
                hash,
                stamp: after,
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
        if validate && doc.state.status != RecordStatus::IncorrectJson {
            let t = Instant::now();
            doc.validate(c);
            metrics.validations += 1;
            metrics.validation_nanos += nanos(t);
        }
        metrics.parse_nanos += nanos(t);
        doc
    }
}
