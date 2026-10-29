//! Validation scheduling, waiting, and explicit runs.

use super::*;


impl Repository {
    pub(super) fn enqueue_validation(&mut self, c: &Collection) -> Result<()> {
        if self.validator.is_none() {
            let writer = match &self.writer {
                Some(writer) => writer.clone(),
                None => self.store()?,
            };
            self.validator = Some(Validator::new(
                writer,
                self.options.background_workers,
                self.options.sqlite_cache_mib,
                self.options.sql_diagnostics,
            )?);
        }
        self.validator.as_ref().unwrap().schedule(c.clone());
        Ok(())
    }

    /// Wait for already scheduled validation. Does not refresh source files.
    pub fn wait_for_validation(&self) -> Result<ValidationMetrics> {
        self.validator
            .as_ref()
            .map_or_else(|| Ok(ValidationMetrics::default()), Validator::wait)
    }

    pub fn validation_metrics(&self) -> ValidationMetrics {
        self.validator
            .as_ref()
            .map_or_else(ValidationMetrics::default, Validator::metrics)
    }

    /// Coalesced collection notifications published after validation commits.
    pub fn poll_validation_updates(&self) -> Result<Vec<String>> {
        self.validator
            .as_ref()
            .map_or_else(|| Ok(Vec::new()), Validator::updates)
    }

    /// Reconcile one collection and explicitly validate all pending cached records.
    pub fn validate_collection(&mut self, name: &str) -> Result<ValidationMetrics> {
        if self.options.cache == CacheMode::Sqlite {
            self.refresh(name)?;
            let c = self.collection(name)?;
            self.enqueue_validation(&c)?;
        } else {
            self.reload()?;
            let c = self.collection(name)?;
            self.direct(&c, None, true)?;
        }
        self.wait_for_validation()
    }

    /// Reconcile and validate every registered collection, including unopened ones.
    /// Uses the same validation policy and cumulative worker metrics as validating
    /// individual collections. With caching disabled, validation runs inline.
    pub fn validate_all(&mut self) -> Result<ValidationMetrics> {
        self.reload()?;
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.validate_collection(&name)?;
        }
        self.wait_for_validation()
    }
}
