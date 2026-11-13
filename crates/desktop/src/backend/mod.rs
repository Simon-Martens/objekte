//! Read-only repository operations shared by the desktop controller.

mod batch;
mod error;
mod export;
pub mod index_lifecycle;
mod ndjson;
mod open;
mod query;
mod record;
pub(crate) mod record_json;

pub use error::{BackendError, BackendErrorKind};
pub use export::{
    export_all_json, export_all_json_on_view, export_selected_json, export_selected_json_on_view,
};
pub use ndjson::{export_query_ndjson, export_query_ndjson_on_view};
pub use open::{
    OpenDiagnostics, OpenSpec, OpenedSession, SessionConfiguration, open, open_incremental,
};
pub use query::{
    MAX_PAGE_ROWS, QueryPage, QuerySpec, build_query, count_exact, count_exact_on_view, query_page,
    query_page_on_view,
};
pub use record::{
    backlink_page, backlink_page_on_view, collection_graph, collection_graph_on_view,
    outgoing_page, outgoing_page_on_view, record_by_id, record_by_id_on_view,
};

#[cfg(test)]
pub(crate) mod test_support;
pub use batch::{BatchReport, BatchSource, BatchSpec, run_batch, run_batch_on_view};
