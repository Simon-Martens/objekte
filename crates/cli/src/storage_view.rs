//! Shared, side-effect-free storage status wording for the TUI and desktop.

use objekte_search_core::index_storage::{
    IndexReadiness, IndexResidency, IndexStorageFailureKind, IndexStorageLocation,
    IndexStoragePolicy, IndexStorageStatus, IndexSynchronization,
};
use objekte_search_core::lifecycle::IndexError;


#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageStatusView {
    pub location: String,
    pub residency: String,
    pub readiness: String,
    pub reconciliation: String,
    pub synchronization: String,
    pub generations: String,
    pub validation: String,
    pub managed_directory: Option<String>,
    pub issue: Option<StorageIssueView>,
    pub validation_warning: Option<StorageIssueView>,
}


#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageIssueView {
    pub kind: IndexStorageFailureKind,
    pub summary: String,
    pub guidance: String,
    pub detail: String,
}


pub fn describe(policy: &IndexStoragePolicy, status: &IndexStorageStatus) -> StorageStatusView {
    let memory = policy.location == IndexStorageLocation::MemoryOnly;
    let location = match &policy.location {
        IndexStorageLocation::MemoryOnly => "Memory only".to_owned(),
        IndexStorageLocation::SystemTemporary => "System temporary folder".to_owned(),
        IndexStorageLocation::RepositoryDirectory => "Repository folder".to_owned(),
        IndexStorageLocation::CustomDirectory(path) => {
            format!("Custom folder: {}", path.display())
        }
    };
    let residency = if memory {
        "Entire index in memory".to_owned()
    } else {
        match (policy.residency, status.readiness) {
            (IndexResidency::FullyResident, IndexReadiness::PreparingFullResidency) => {
                "Preparing full residency".to_owned()
            }
            (IndexResidency::FullyResident, _) => "Entire index in memory".to_owned(),
            (IndexResidency::OnDemand, _) => "On-demand".to_owned(),
        }
    };
    let readiness = match status.readiness {
        IndexReadiness::Building => "Building",
        IndexReadiness::Searchable => "Searchable",
        IndexReadiness::PreparingFullResidency => "Preparing full residency",
        IndexReadiness::FullyResidentReady => "Ready",
    }
    .to_owned();
    let reconciliation = if status.source_reconciliation_complete {
        "Source files current"
    } else {
        "Checking source files"
    }
    .to_owned();
    let synchronization = if memory {
        "No index files are read or written"
    } else {
        match status.synchronization {
            IndexSynchronization::MemoryOnly => "No index files are read or written",
            IndexSynchronization::Pending => "Changes pending",
            IndexSynchronization::Synchronizing => "Synchronizing",
            IndexSynchronization::Synchronized => "Synchronized",
            IndexSynchronization::Failed => "Synchronization failed",
        }
    }
    .to_owned();
    let generations = if memory {
        String::new()
    } else {
        match status.durable_generation {
            Some(durable) => format!(
                "Generation {} (durable {durable})",
                status.current_generation
            ),
            None => format!("Generation {} (not yet durable)", status.current_generation),
        }
    };
    let validation = if memory {
        String::new()
    } else {
        format!(
            "{} fully validated, {} trusted stamps, {} stamps written",
            status.segments_full_validated,
            status.segments_stamp_trusted,
            status.validation_stamps_written,
        )
    };
    let issue = if memory {
        None
    } else {
        status.last_error.as_ref().map(|detail| {
            describe_failure(
                status
                    .last_error_kind
                    .unwrap_or(IndexStorageFailureKind::Other),
                detail,
            )
        })
    };
    let validation_warning = if memory {
        None
    } else {
        status.validation_stamp_error.as_ref().map(|detail| {
            let mut warning = describe_failure(
                status
                    .validation_stamp_error_kind
                    .unwrap_or(IndexStorageFailureKind::Other),
                detail,
            );
            warning.summary = "Index validated; progress stamp could not be saved".to_owned();
            warning.guidance =
                "The index is usable; the next open may validate more slowly.".to_owned();
            warning
        })
    };
    StorageStatusView {
        location,
        residency,
        readiness,
        reconciliation,
        synchronization,
        generations,
        validation,
        managed_directory: (!memory)
            .then(|| {
                status
                    .managed_directory
                    .as_ref()
                    .map(|path| path.display().to_string())
            })
            .flatten(),
        issue,
        validation_warning,
    }
}

pub fn describe_failure(kind: IndexStorageFailureKind, detail: &str) -> StorageIssueView {
    let (summary, guidance) = match kind {
        IndexStorageFailureKind::Access => (
            "Storage access error",
            "Check folder permissions or free space, then retry.",
        ),
        IndexStorageFailureKind::Corrupt => (
            "Index data is damaged",
            "Rebuild the index; source files are untouched.",
        ),
        IndexStorageFailureKind::Incompatible => (
            "Index format or configuration changed",
            "Rebuild the index.",
        ),
        IndexStorageFailureKind::Conflict => (
            "Index is busy",
            "Another process is updating this index. Retry in a moment.",
        ),
        IndexStorageFailureKind::Resource => (
            "Storage resource limit",
            "Free space or reduce the index size, then retry.",
        ),
        IndexStorageFailureKind::Other => (
            "Index storage error",
            "Retry; if it persists, use Details to report the error.",
        ),
    };
    StorageIssueView {
        kind,
        summary: summary.to_owned(),
        guidance: guidance.to_owned(),
        detail: detail.to_owned(),
    }
}

/// Preserve typed recovery guidance when an open fails before a status exists.
pub fn format_index_error(error: &IndexError) -> String {
    let issue = describe_failure(IndexStorageFailureKind::from(error), &error.to_string());
    format!(
        "{}: {} Details: {}",
        issue.summary, issue.guidance, issue.detail
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    fn disk_status() -> IndexStorageStatus {
        IndexStorageStatus {
            readiness: IndexReadiness::Searchable,
            source_reconciliation_complete: false,
            current_generation: 9,
            durable_generation: Some(8),
            synchronization: IndexSynchronization::Pending,
            last_error: None,
            last_error_kind: None,
            managed_directory: Some("/tmp/objekte-indexes/repository".into()),
            segments_full_validated: 2,
            segments_stamp_trusted: 3,
            validation_stamps_written: 1,
            validation_stamp_error: None,
            validation_stamp_error_kind: None,
        }
    }

    fn disk_policy() -> IndexStoragePolicy {
        IndexStoragePolicy {
            location: IndexStorageLocation::RepositoryDirectory,
            residency: IndexResidency::OnDemand,
        }
    }

    #[test]
    fn memory_hides_disk_details_even_from_an_inconsistent_status() {
        let mut status = disk_status();
        status.last_error = Some("old disk error".into());
        status.validation_stamp_error = Some("old stamp error".into());
        let view = describe(&IndexStoragePolicy::default(), &status);
        assert_eq!(view.synchronization, "No index files are read or written");
        assert!(view.generations.is_empty());
        assert!(view.validation.is_empty());
        assert!(view.managed_directory.is_none());
        assert!(view.issue.is_none());
        assert!(view.validation_warning.is_none());
    }

    #[test]
    fn disk_status_covers_readiness_generations_and_validation() {
        let status = disk_status();
        let view = describe(&disk_policy(), &status);
        assert_eq!(view.reconciliation, "Checking source files");
        assert_eq!(view.synchronization, "Changes pending");
        assert_eq!(view.generations, "Generation 9 (durable 8)");
        assert_eq!(
            view.validation,
            "2 fully validated, 3 trusted stamps, 1 stamps written"
        );
        assert!(view.managed_directory.unwrap().contains("objekte-indexes"));
        let mut status = status;
        status.readiness = IndexReadiness::PreparingFullResidency;
        status.synchronization = IndexSynchronization::Synchronizing;
        status.durable_generation = None;
        let view = describe(
            &IndexStoragePolicy {
                residency: IndexResidency::FullyResident,
                ..disk_policy()
            },
            &status,
        );
        assert_eq!(view.residency, "Preparing full residency");
        assert_eq!(view.generations, "Generation 9 (not yet durable)");
        assert_eq!(view.synchronization, "Synchronizing");
    }

    #[test]
    fn failure_kinds_have_distinct_guidance_and_stamp_warning_is_nonfatal() {
        let mut status = disk_status();
        for kind in [
            IndexStorageFailureKind::Access,
            IndexStorageFailureKind::Corrupt,
            IndexStorageFailureKind::Incompatible,
            IndexStorageFailureKind::Conflict,
            IndexStorageFailureKind::Resource,
            IndexStorageFailureKind::Other,
        ] {
            status.last_error = Some("details".into());
            status.last_error_kind = Some(kind);
            let view = describe(&disk_policy(), &status);
            let issue = view.issue.unwrap();
            assert_eq!(issue.kind, kind);
            assert!(!issue.summary.is_empty());
            assert!(!issue.guidance.is_empty());
            assert_eq!(issue.detail, "details");
        }
        status.last_error = None;
        status.validation_stamp_error = Some("stamp lock held".into());
        status.validation_stamp_error_kind = Some(IndexStorageFailureKind::Conflict);
        let view = describe(&disk_policy(), &status);
        assert!(view.issue.is_none());
        assert_eq!(
            view.validation_warning.unwrap().kind,
            IndexStorageFailureKind::Conflict
        );
    }

    #[test]
    fn custom_path_is_preserved_without_truncating_it() {
        let path = format!("/tmp/{}", "long-folder/".repeat(25));
        let policy = IndexStoragePolicy {
            location: IndexStorageLocation::CustomDirectory(path.clone().into()),
            residency: IndexResidency::OnDemand,
        };
        assert!(describe(&policy, &disk_status()).location.ends_with(&path));
    }

    #[test]
    fn typed_open_error_keeps_recovery_guidance() {
        let message = format_index_error(&IndexError::InvalidCache("bad manifest".into()));
        assert!(message.contains("Index data is damaged"));
        assert!(message.contains("Rebuild the index"));
        assert!(message.contains("bad manifest"));
        let incompatible = format_index_error(&IndexError::IncompatibleConfiguration("v2".into()));
        assert!(incompatible.contains("Index format or configuration changed"));
        assert!(incompatible.contains("Rebuild the index"));
    }
}
