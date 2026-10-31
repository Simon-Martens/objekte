//! Link and backlink state for the selected record in the detail view.

use super::state::{App, Focus, Hit};
use objekte_core::{Record, RecordRelation};


/// Which part of the detail view consumes Up and Down.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DetailSection {
    Fields,
    Links,
    Backlinks,
}


/// Progress of the complete backlink lookup for one record.
pub(super) enum BacklinkStatus {
    None,
    Loading,
    Complete,
    Error(String),
}


/// Record identity used to start an asynchronous backlink lookup.
pub(super) struct LinkTarget {
    pub(super) collection: String,
    pub(super) id: String,
}


/// Current link tables, selection, and visited-record history.
pub(super) struct DetailLinks {
    pub(super) override_hit: Option<Hit>,
    history: Vec<Option<Hit>>,
    pub(super) outgoing: Vec<RecordRelation>,
    pub(super) incoming: Vec<RecordRelation>,
    pub(super) status: BacklinkStatus,
    pub(super) section: DetailSection,
    pub(super) outgoing_selected: usize,
    pub(super) incoming_selected: usize,
    pub(super) generation: u64,
    pub(super) pending: Option<LinkTarget>,
}


impl Default for DetailLinks {
    fn default() -> Self {
        Self {
            override_hit: None,
            history: Vec::new(),
            outgoing: Vec::new(),
            incoming: Vec::new(),
            status: BacklinkStatus::None,
            section: DetailSection::Fields,
            outgoing_selected: 0,
            incoming_selected: 0,
            generation: 0,
            pending: None,
        }
    }
}


impl DetailLinks {
    pub(super) fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.override_hit = None;
        self.history.clear();
        self.outgoing.clear();
        self.incoming.clear();
        self.status = BacklinkStatus::None;
        self.section = DetailSection::Fields;
        self.outgoing_selected = 0;
        self.incoming_selected = 0;
        self.pending = None;
    }

    pub(super) fn selected_relation(&self) -> Option<&RecordRelation> {
        match self.section {
            DetailSection::Fields => None,
            DetailSection::Links => self.outgoing.get(self.outgoing_selected),
            DetailSection::Backlinks => self.incoming.get(self.incoming_selected),
        }
    }

    pub(super) fn move_selection(&mut self, forwards: bool) {
        let (selection, length) = match self.section {
            DetailSection::Fields => return,
            DetailSection::Links => (&mut self.outgoing_selected, self.outgoing.len()),
            DetailSection::Backlinks => (&mut self.incoming_selected, self.incoming.len()),
        };
        *selection = if forwards {
            (*selection + 1).min(length.saturating_sub(1))
        } else {
            selection.saturating_sub(1)
        };
    }

    pub(super) fn cycle_section(&mut self, backwards: bool) {
        self.section = match (self.section, backwards) {
            (DetailSection::Fields, false) | (DetailSection::Backlinks, true) => {
                DetailSection::Links
            }
            (DetailSection::Links, false) | (DetailSection::Fields, true) => {
                DetailSection::Backlinks
            }
            (DetailSection::Backlinks, false) | (DetailSection::Links, true) => {
                DetailSection::Fields
            }
        };
    }
}


impl App<'_> {
    pub(super) fn current_detail_hit(&self) -> Option<&Hit> {
        self.links
            .override_hit
            .as_ref()
            .or_else(|| self.hits.get(self.selected))
    }

    pub(super) fn open_detail(&mut self) {
        if self.hits.is_empty() {
            return;
        }
        self.links.reset();
        self.focus = Focus::Detail;
        self.detail_scroll = 0;
        self.detail_position();
        self.prepare_detail_links();
    }

    pub(super) fn prepare_detail_links(&mut self) {
        self.links.generation = self.links.generation.wrapping_add(1);
        self.links.outgoing.clear();
        self.links.incoming.clear();
        self.links.outgoing_selected = 0;
        self.links.incoming_selected = 0;
        self.links.pending = None;
        let Some(hit) = self.current_detail_hit() else {
            self.links.status = BacklinkStatus::None;
            return;
        };
        let record = hit.record();
        let target = LinkTarget {
            collection: record.collection.clone(),
            id: record.id.clone(),
        };
        match self.repo.references_in_record(record) {
            Ok(edges) => self.links.outgoing = edges,
            Err(error) => {
                self.links.status = BacklinkStatus::Error(error.to_string());
                return;
            }
        }
        if self
            .repo
            .model()
            .graph
            .edges
            .iter()
            .any(|edge| edge.target == target.collection)
        {
            self.links.status = BacklinkStatus::Loading;
            self.links.pending = Some(target);
        } else {
            self.links.status = BacklinkStatus::Complete;
        }
    }

    pub(super) fn add_backlink_record(&mut self, record: &Record) {
        let Some(hit) = self.current_detail_hit() else {
            return;
        };
        let target = hit.record();
        let (collection, id) = (target.collection.clone(), target.id.clone());
        if let Ok(edges) = self.repo.references_in_record(record) {
            self.links.incoming.extend(
                edges
                    .into_iter()
                    .filter(|edge| edge.target_collection == collection && edge.target_id == id),
            );
        }
    }

    pub(super) fn open_selected_relation(&mut self) {
        let Some(edge) = self.links.selected_relation() else {
            self.message = "Select a link or backlink first.".into();
            return;
        };
        let (collection, id) = if self.links.section == DetailSection::Links {
            (&edge.target_collection, &edge.target_id)
        } else {
            (&edge.source_collection, &edge.source_id)
        };
        let (collection, id) = (collection.clone(), id.clone());
        match self.repo.get_record(&collection, &id) {
            Ok(Some(record)) => {
                self.links.history.push(self.links.override_hit.take());
                self.links.override_hit = Some(Hit::Filter(record));
                self.links.section = DetailSection::Fields;
                self.detail_scroll = 0;
                self.prepare_detail_links();
                self.message =
                    format!("Opened {collection}/{id}. Backspace returns to the previous item.");
            }
            Ok(None) => self.message = format!("Missing target: {collection}/{id}"),
            Err(error) => self.message = format!("Cannot open {collection}/{id}: {error}"),
        }
    }

    pub(super) fn leave_detail(&mut self) {
        if let Some(previous) = self.links.history.pop() {
            self.links.override_hit = previous;
            self.links.section = DetailSection::Fields;
            self.detail_scroll = 0;
            self.prepare_detail_links();
            self.detail_position();
        } else {
            self.close_detail();
        }
    }

    pub(super) fn close_detail(&mut self) {
        self.links.reset();
        self.focus = Focus::Results;
        self.pending_detail = None;
        self.message.clear();
    }
}


#[cfg(test)]
#[path = "navigation.test.rs"]
mod tests;
