//! Validation is computed from authoritative source files on demand.

use super::*;


impl Repository {
    pub fn validate_collection(&mut self, name: &str) -> Result<ValidationMetrics> {
        self.reload()?;
        let c = self.collection(name)?;
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
        self.reload()?;
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
