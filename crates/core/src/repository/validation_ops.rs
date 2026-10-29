//! Validation is computed from authoritative source files on demand.

use super::*;


impl Repository {
    pub fn wait_for_validation(&self) -> Result<ValidationMetrics> {
        Ok(ValidationMetrics::default())
    }

    pub fn validation_metrics(&self) -> ValidationMetrics {
        ValidationMetrics::default()
    }

    pub fn poll_validation_updates(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    pub fn validate_collection(&mut self, name: &str) -> Result<ValidationMetrics> {
        self.reload()?;
        let c = self.collection(name)?;
        if self.options.cache == CacheMode::Segments {
            self.sync_segments(&c, true)?;
        }
        let before = self.metrics.validations;
        self.direct(&c, None, true)?;
        let validated = self.metrics.validations - before;
        Ok(ValidationMetrics {
            validated,
            applied: validated,
            ..Default::default()
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
        }
        Ok(total)
    }
}
