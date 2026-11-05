//! Validation is computed from authoritative source files on demand.

use super::*;


impl Repository {
    pub fn validate_collection(&mut self, name: &str) -> Result<ValidationMetrics> {
        if let Some(mut selected) = self.selected_route(&[name.to_owned()])? {
            let (c, snapshot) = selected.pop().expect("selected collection exists");
            if c.schema.is_none() {
                return Ok(ValidationMetrics::default());
            }
            let before = self.metrics.validations;
            let started = Instant::now();
            let mut bytes = Vec::new();
            for source in snapshot.sources.iter() {
                self.selected_record(&c, source, &mut bytes, true)?;
            }
            if self.options.freshness != Freshness::MetadataOnOpen {
                let current = self.source_descriptors(name)?;
                self.metrics.enumerations += 1;
                if current != *snapshot.sources {
                    return Err(Error::Unstable(c.path));
                }
            }
            let validated = self.metrics.validations - before;
            return Ok(ValidationMetrics {
                validated,
                applied: validated,
                validation_nanos: nanos(started),
            });
        }
        let c = self.collection(name)?;
        if c.schema.is_none() {
            return Ok(ValidationMetrics::default());
        }
        let before = self.metrics.validations;
        let started = Instant::now();
        self.direct(&c, None, true)?;
        let validated = self.metrics.validations - before;
        Ok(ValidationMetrics {
            validated,
            applied: validated,
            validation_nanos: nanos(started),
        })
    }

    pub fn validate_all(&mut self) -> Result<ValidationMetrics> {
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        let mut total = ValidationMetrics::default();
        for name in names {
            let metrics = self.validate_collection(&name)?;
            total.validated += metrics.validated;
            total.applied += metrics.applied;
            total.validation_nanos += metrics.validation_nanos;
        }
        Ok(total)
    }
}
