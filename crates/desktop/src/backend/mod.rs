//! Read-only repository operations shared by the desktop controller.

mod batch;
mod error;
mod export;
mod open;
mod query;
mod record;

pub use error::{BackendError, BackendErrorKind};
pub use export::{export_all_json, export_selected_json};
pub use open::{OpenDiagnostics, OpenSpec, OpenedSession, open};
pub use query::{MAX_PAGE_ROWS, QueryPage, QuerySpec, build_query, count_exact, query_page};
pub use record::{backlink_page, collection_graph, outgoing_page, record_by_id};

#[cfg(test)]
mod test_support;
pub use batch::{BatchReport, BatchSource, BatchSpec, run_batch};
