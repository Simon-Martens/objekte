//! Stable request scope shared by planning and execution.

use crate::prepared::PreparedQuery;

pub(crate) struct RequestExecution<'a> {
    pub(crate) prepared: &'a PreparedQuery,
}
