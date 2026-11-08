//! Identity guards for asynchronous work over mutable index publications.
//!
//! An index revision is only ordered inside its own instance. A cache load
//! creates a new instance, even if its numeric revision equals the old one.

use objekte_search_core::lifecycle::{IndexRevision, IndexView};

use crate::backend::QueryPage;

/// One publication installed in one desktop repository session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicationKey {
    pub session_generation: u64,
    pub instance_id: u64,
    pub revision: IndexRevision,
}

impl PublicationKey {
    pub fn from_view(session_generation: u64, view: &IndexView) -> Self {
        Self {
            session_generation,
            instance_id: view.instance_id(),
            revision: view.revision(),
        }
    }

    /// A progress event or save does not change this key. Only publication
    /// changes (or replacement of the whole index instance) invalidate data.
    pub fn is_current(self, current: Self) -> bool {
        self == current
    }

    pub fn owns_page(self, page: &QueryPage) -> bool {
        self.instance_id == page.instance_id && self.revision == page.revision
    }
}

/// The executed query generation must also match before a page or count may
/// update the visible result context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResultContextKey {
    pub publication: PublicationKey,
    pub query_generation: u64,
}

impl ResultContextKey {
    pub fn owns_response(self, response: Self) -> bool {
        self == response
    }

    pub fn owns_page(self, response: Self, page: &QueryPage) -> bool {
        self.owns_response(response) && self.publication.owns_page(page)
    }
}

/// Every tab job also needs its tab-local operation generation. A result from
/// a closed or refreshed tab must not update another tab with the same record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TabJobKey {
    pub publication: PublicationKey,
    pub tab_id: u64,
    pub operation_generation: u64,
}

impl TabJobKey {
    pub fn owns_response(self, response: Self) -> bool {
        self == response
    }
}

#[cfg(test)]
#[path = "lifecycle_navigation.test.rs"]
mod tests;
