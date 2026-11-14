//! Shared wording for explicit index operations and typed storage failures.

use objekte_search_core::index_storage::{IndexStorageFailureKind, IndexStorageTarget};
use objekte_search_core::lifecycle::IndexError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageIssueView {
    pub kind: IndexStorageFailureKind,
    pub summary: String,
    pub guidance: String,
    pub detail: String,
}

pub fn target_label(target: &IndexStorageTarget) -> String {
    match target {
        IndexStorageTarget::SystemTemporary => "System temporary folder".into(),
        IndexStorageTarget::RepositoryDirectory => "Repository folder".into(),
        IndexStorageTarget::CustomDirectory(path) => format!("Custom folder: {}", path.display()),
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
            "Omit the load option to rebuild; source files are untouched.",
        ),
        IndexStorageFailureKind::Incompatible => (
            "Index format or configuration changed",
            "Omit the load option to rebuild.",
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

    #[test]
    fn targets_have_distinct_labels_and_custom_path_is_preserved() {
        assert_eq!(
            target_label(&IndexStorageTarget::SystemTemporary),
            "System temporary folder"
        );
        assert_eq!(
            target_label(&IndexStorageTarget::RepositoryDirectory),
            "Repository folder"
        );
        let custom =
            IndexStorageTarget::CustomDirectory(format!("/tmp/{}", "long/".repeat(24)).into());
        assert!(target_label(&custom).contains(&"long/".repeat(24)));
    }

    #[test]
    fn failure_kinds_have_distinct_guidance() {
        let access = describe_failure(IndexStorageFailureKind::Access, "denied");
        let corrupt = describe_failure(IndexStorageFailureKind::Corrupt, "broken");
        assert_ne!(access.summary, corrupt.summary);
        assert!(corrupt.guidance.contains("Omit the load option"));
        assert_eq!(access.detail, "denied");
    }
}
