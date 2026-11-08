//! Read-only repository operations shared by the desktop controller.

mod error;
mod open;
mod query;
mod record;

pub use error::{BackendError, BackendErrorKind};
pub use open::{OpenDiagnostics, OpenSpec, OpenedSession, open};
pub use query::{MAX_PAGE_ROWS, QueryPage, QuerySpec, build_query, count_exact, query_page};
pub use record::{backlink_page, collection_graph, outgoing_page, record_by_id};

#[cfg(test)]
mod test_support;
