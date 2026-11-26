//! Relation traversal work accounting and query-wide limits.
use std::sync::Arc;

use crate::inventory::ActiveRecords;

use crate::model::{CollectionId, RecordSlot};

const MAX_QUERY_EDGE_VISITS: u64 = 32_000_000;
const MAX_QUERY_BACKLINK_PROBES: u64 = 32_000_000;
const MAX_QUERY_RECORD_READS: u64 = 5_000_000;
pub(super) const MAX_QUERY_RELATION_BYTES: u64 = 1024 * 1024 * 1024;

/// Physical work caused by relation traversal, separate from the source scan.
#[derive(Clone, Default)]
pub(crate) struct RelationWork {
    pub source_reads: u64,
    pub target_reads: u64,
    pub target_cache_hits: u64,
    pub edge_visits: u64,
    pub backlink_probe_visits: u64,
    pub bytes_read: u64,
    execution: Option<crate::query_execution::streaming::ExecutionContext>,
    active: Option<Arc<Vec<ActiveRecords>>>,
}

fn charge(counter: &mut u64, amount: u64, limit: u64, name: &str) -> Result<(), String> {
    *counter = counter
        .checked_add(amount)
        .ok_or_else(|| format!("relation {name} overflow"))?;
    if *counter > limit {
        return Err(format!("relation query exceeds {limit} {name}"));
    }
    Ok(())
}

impl RelationWork {
    pub(crate) fn attach(
        &mut self,
        execution: Option<&crate::query_execution::streaming::ExecutionContext>,
    ) {
        self.execution = execution.cloned();
        self.active = execution.and_then(|context| context.active_records_arc());
    }

    pub(super) fn execution(&self) -> Option<&crate::query_execution::streaming::ExecutionContext> {
        self.execution.as_ref()
    }

    pub(super) fn has_active_domains(&self) -> bool {
        self.active.is_some()
    }

    pub(super) fn is_active(
        &self,
        collection: CollectionId,
        slot: RecordSlot,
    ) -> Result<bool, String> {
        match &self.active {
            Some(domains) => Ok(domains[collection.0 as usize].contains(slot)?),
            None => Ok(true),
        }
    }

    pub(super) fn filter_slot(
        &self,
        collection: CollectionId,
        slot: Option<RecordSlot>,
    ) -> Result<Option<RecordSlot>, String> {
        match slot {
            Some(slot) if self.is_active(collection, slot)? => Ok(Some(slot)),
            _ => Ok(None),
        }
    }

    pub(super) fn before_read(&self, bytes: u64, target: bool) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.check_string()?;
            let limit = if target {
                execution.limits.target_record_bytes
            } else {
                execution.limits.source_record_bytes
            };
            if bytes > limit {
                return Err(execution.fail(
                    crate::query_execution::streaming::ExecutionErrorKind::Resource,
                    "relation record exceeds source read allowance",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn edge(&mut self) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_edges(1)?;
        }
        charge(
            &mut self.edge_visits,
            1,
            MAX_QUERY_EDGE_VISITS,
            "edge visits",
        )
    }

    pub(super) fn backlink_probe(&mut self) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_edges(1)?;
        }
        charge(
            &mut self.backlink_probe_visits,
            1,
            MAX_QUERY_BACKLINK_PROBES,
            "backlink probes",
        )
    }

    pub(super) fn target_read(&mut self, bytes: u64) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_reads(1)?;
            execution.charge_bytes(bytes)?;
        }
        charge(
            &mut self.target_reads,
            1,
            MAX_QUERY_RECORD_READS,
            "target reads",
        )?;
        charge(
            &mut self.bytes_read,
            bytes,
            MAX_QUERY_RELATION_BYTES,
            "source bytes",
        )
    }

    pub(super) fn source_read(&mut self, bytes: u64) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_reads(1)?;
            execution.charge_bytes(bytes)?;
        }
        charge(
            &mut self.source_reads,
            1,
            MAX_QUERY_RECORD_READS,
            "source reads",
        )?;
        charge(
            &mut self.bytes_read,
            bytes,
            MAX_QUERY_RELATION_BYTES,
            "source bytes",
        )
    }
}

/// Combine bounded scanner chunks into one query-wide relation work budget.
pub(crate) fn check_query_work_budget(
    counters: &crate::measurement::WorkCounters,
) -> Result<(), String> {
    for (actual, limit, name) in [
        (
            counters.relation_edge_visits,
            MAX_QUERY_EDGE_VISITS,
            "edge visits",
        ),
        (
            counters.relation_backlink_probe_visits,
            MAX_QUERY_BACKLINK_PROBES,
            "backlink probes",
        ),
        (
            counters.relation_target_reads,
            MAX_QUERY_RECORD_READS,
            "target reads",
        ),
        (
            counters.relation_source_reads,
            MAX_QUERY_RECORD_READS,
            "source reads",
        ),
        (
            counters.relation_bytes_read,
            MAX_QUERY_RELATION_BYTES,
            "source bytes",
        ),
    ] {
        if actual > limit {
            return Err(format!("relation query exceeds {limit} {name}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
