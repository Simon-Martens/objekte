//! Window-local tab placement. Query execution remains controller-owned.

use super::model::{RecordTabSummary, SearchDraft};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pane {
    Primary,
    Secondary,
}

impl Default for Pane {
    fn default() -> Self {
        Self::Primary
    }
}

impl Pane {
    pub fn other(self) -> Self {
        match self {
            Self::Primary => Self::Secondary,
            Self::Secondary => Self::Primary,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabId {
    Search(u64),
    Record(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchKind {
    Table,
    Query,
}

#[derive(Clone, Debug)]
pub struct WorkspaceTab {
    pub id: TabId,
    pub title: String,
    pub draft: Option<SearchDraft>,
    pub search_kind: Option<SearchKind>,
    pub query_number: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct TabGroup {
    pub tabs: Vec<WorkspaceTab>,
    pub active: Option<TabId>,
}

#[derive(Clone, Debug, Default)]
pub struct Workspace {
    pub primary: TabGroup,
    pub secondary: TabGroup,
    next_search_id: u64,
    next_query_number: u64,
    pub current_search: Option<TabId>,
    pub focused_pane: Pane,
    last_active_record: Option<u64>,
}

impl Workspace {
    pub fn group(&self, pane: Pane) -> &TabGroup {
        match pane {
            Pane::Primary => &self.primary,
            Pane::Secondary => &self.secondary,
        }
    }

    fn group_mut(&mut self, pane: Pane) -> &mut TabGroup {
        match pane {
            Pane::Primary => &mut self.primary,
            Pane::Secondary => &mut self.secondary,
        }
    }

    pub fn pane_of(&self, id: TabId) -> Option<Pane> {
        [Pane::Primary, Pane::Secondary]
            .into_iter()
            .find(|pane| self.group(*pane).tabs.iter().any(|tab| tab.id == id))
    }

    pub fn tab(&self, id: TabId) -> Option<&WorkspaceTab> {
        self.pane_of(id)
            .and_then(|pane| self.group(pane).tabs.iter().find(|tab| tab.id == id))
    }

    pub fn focused_active_tab(&self) -> Option<TabId> {
        self.group(self.focused_pane).active
    }

    pub fn adjacent_tab(&self, reverse: bool) -> Option<TabId> {
        let group = self.group(self.focused_pane);
        if group.tabs.is_empty() {
            return None;
        }
        let current = group
            .active
            .and_then(|active| group.tabs.iter().position(|tab| tab.id == active))
            .unwrap_or(0);
        let next = if reverse {
            (current + group.tabs.len() - 1) % group.tabs.len()
        } else {
            (current + 1) % group.tabs.len()
        };
        Some(group.tabs[next].id)
    }

    pub fn opened(&mut self, collection: Option<String>) -> TabId {
        *self = Self::default();
        self.open_table(collection)
    }

    pub fn open_table(&mut self, collection: Option<String>) -> TabId {
        if let Some(id) = self
            .primary
            .tabs
            .iter()
            .chain(&self.secondary.tabs)
            .find(|tab| {
                tab.search_kind == Some(SearchKind::Table)
                    && tab
                        .draft
                        .as_ref()
                        .is_some_and(|draft| draft.collection == collection)
            })
            .map(|tab| tab.id)
        {
            self.activate(id);
            return id;
        }
        let draft = SearchDraft {
            collection: collection.clone(),
            ..SearchDraft::default()
        };
        self.insert_search(draft, SearchKind::Table, Pane::Primary)
    }

    pub fn open_query(&mut self, draft: SearchDraft) -> TabId {
        self.open_query_in(draft, Pane::Primary)
    }

    pub fn open_query_in(&mut self, draft: SearchDraft, pane: Pane) -> TabId {
        self.insert_search(draft, SearchKind::Query, pane)
    }

    pub fn duplicate_query(&mut self, id: TabId) -> Option<TabId> {
        let pane = self.pane_of(id)?;
        let group = self.group(pane);
        let index = group.tabs.iter().position(|tab| tab.id == id)?;
        let draft = group.tabs[index].draft.clone()?;
        let duplicate = self.open_query_in(draft, pane);
        self.move_tab(duplicate, pane, index + 1);
        Some(duplicate)
    }

    fn insert_search(&mut self, draft: SearchDraft, kind: SearchKind, pane: Pane) -> TabId {
        let id = TabId::Search(self.next_search_id);
        self.next_search_id = self.next_search_id.wrapping_add(1);
        let query_number = if kind == SearchKind::Query {
            self.next_query_number += 1;
            Some(self.next_query_number)
        } else {
            None
        };
        let title = search_title(&draft, kind, query_number);
        let group = self.group_mut(pane);
        group.tabs.push(WorkspaceTab {
            id,
            title,
            draft: Some(draft),
            search_kind: Some(kind),
            query_number,
        });
        group.active = Some(id);
        self.current_search = Some(id);
        self.focused_pane = pane;
        id
    }

    pub fn update_current_search(&mut self, draft: SearchDraft) {
        let Some(id) = self.current_search else {
            return;
        };
        let Some(pane) = self.pane_of(id) else { return };
        let Some(tab) = self
            .group_mut(pane)
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id)
        else {
            return;
        };
        if tab.search_kind == Some(SearchKind::Table)
            && (!draft.full_text.is_empty() || draft.filter_enabled && !draft.structured.is_empty())
        {
            tab.search_kind = Some(SearchKind::Query);
        }
        tab.title = search_title(
            &draft,
            tab.search_kind.unwrap_or(SearchKind::Query),
            tab.query_number,
        );
        tab.draft = Some(draft);
    }

    pub fn activate(&mut self, id: TabId) -> bool {
        let Some(pane) = self.pane_of(id) else {
            return false;
        };
        self.group_mut(pane).active = Some(id);
        self.focused_pane = pane;
        if matches!(id, TabId::Search(_)) {
            self.current_search = Some(id);
        }
        true
    }

    pub fn move_tab(&mut self, id: TabId, destination: Pane, at: usize) -> bool {
        let Some(source) = self.pane_of(id) else {
            return false;
        };
        let source_group = self.group_mut(source);
        let old = source_group
            .tabs
            .iter()
            .position(|tab| tab.id == id)
            .unwrap();
        let tab = source_group.tabs.remove(old);
        if source_group.active == Some(id) {
            source_group.active = source_group
                .tabs
                .get(old.min(source_group.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id);
        }
        let destination_group = self.group_mut(destination);
        let at = if source == destination && old < at {
            at.saturating_sub(1)
        } else {
            at
        };
        destination_group
            .tabs
            .insert(at.min(destination_group.tabs.len()), tab);
        destination_group.active = Some(id);
        self.focused_pane = destination;
        true
    }

    pub fn close(&mut self, id: TabId) -> bool {
        let Some(pane) = self.pane_of(id) else {
            return false;
        };
        let group = self.group_mut(pane);
        let index = group.tabs.iter().position(|tab| tab.id == id).unwrap();
        group.tabs.remove(index);
        if group.active == Some(id) {
            group.active = group
                .tabs
                .get(index.min(group.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id);
        }
        if self.current_search == Some(id) {
            self.current_search = self
                .primary
                .tabs
                .iter()
                .chain(&self.secondary.tabs)
                .find(|tab| matches!(tab.id, TabId::Search(_)))
                .map(|tab| tab.id);
        }
        if self.focused_pane == pane && self.group(pane).active.is_none() {
            self.focused_pane = pane.other();
        }
        true
    }

    pub fn sync_records(&mut self, records: &[RecordTabSummary], active: Option<u64>) {
        let ids = records.iter().map(|tab| tab.id).collect::<Vec<_>>();
        for pane in [Pane::Primary, Pane::Secondary] {
            let stale = self
                .group(pane)
                .tabs
                .iter()
                .filter_map(|tab| match tab.id {
                    TabId::Record(id) if !ids.contains(&id) => Some(tab.id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            for id in stale {
                self.close(id);
            }
        }
        for record in records {
            let id = TabId::Record(record.id);
            if let Some(pane) = self.pane_of(id) {
                if let Some(tab) = self
                    .group_mut(pane)
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == id)
                {
                    tab.title = record.title.clone();
                }
            } else {
                self.secondary.tabs.push(WorkspaceTab {
                    id,
                    title: record.title.clone(),
                    draft: None,
                    search_kind: None,
                    query_number: None,
                });
            }
        }
        if active != self.last_active_record {
            if let Some(id) = active {
                self.activate(TabId::Record(id));
            }
        }
        self.last_active_record = active;
    }
}

fn search_title(draft: &SearchDraft, kind: SearchKind, query_number: Option<u64>) -> String {
    if kind == SearchKind::Table {
        draft
            .collection
            .clone()
            .unwrap_or_else(|| "All records".into())
    } else {
        let blank = draft.full_text.trim().is_empty()
            && (!draft.filter_enabled || draft.structured.trim().is_empty());
        if blank {
            return format!("Query {}", query_number.unwrap_or(1));
        }
        let title = super::preferences::generated_title(draft);
        if title.trim().is_empty() {
            "Query".into()
        } else {
            title
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_tabs_keep_independent_collection_and_query_drafts() {
        let mut workspace = Workspace::default();
        let all = workspace.opened(None);
        let docs = workspace.open_table(Some("docs".into()));
        workspace.update_current_search(SearchDraft {
            full_text: "needle".into(),
            collection: Some("docs".into()),
            ..Default::default()
        });
        assert_eq!(
            workspace
                .tab(all)
                .unwrap()
                .draft
                .as_ref()
                .unwrap()
                .collection,
            None
        );
        assert_eq!(
            workspace
                .tab(docs)
                .unwrap()
                .draft
                .as_ref()
                .unwrap()
                .full_text,
            "needle"
        );
        assert_eq!(
            workspace.tab(docs).unwrap().search_kind,
            Some(SearchKind::Query)
        );
        assert_ne!(workspace.open_table(Some("docs".into())), docs);
    }

    #[test]
    fn moving_tabs_reorders_and_preserves_active_selection() {
        let mut workspace = Workspace::default();
        let first = workspace.opened(None);
        let second = workspace.open_query(SearchDraft::default());
        assert!(workspace.move_tab(second, Pane::Primary, 0));
        assert_eq!(workspace.primary.tabs[0].id, second);
        assert!(workspace.move_tab(first, Pane::Secondary, 0));
        assert_eq!(workspace.secondary.active, Some(first));
        assert_eq!(workspace.primary.active, Some(second));
        assert_eq!(workspace.focused_pane, Pane::Secondary);
    }

    #[test]
    fn duplicate_query_preserves_draft_and_inserts_beside_source() {
        let mut workspace = Workspace::default();
        workspace.opened(None);
        let original = workspace.open_query_in(
            SearchDraft {
                full_text: "orange".into(),
                ..Default::default()
            },
            Pane::Secondary,
        );
        let duplicate = workspace.duplicate_query(original).unwrap();
        assert_eq!(
            workspace
                .secondary
                .tabs
                .iter()
                .map(|tab| tab.id)
                .collect::<Vec<_>>(),
            vec![original, duplicate]
        );
        assert_eq!(
            workspace
                .tab(duplicate)
                .unwrap()
                .draft
                .as_ref()
                .unwrap()
                .full_text,
            "orange"
        );
        assert_eq!(workspace.secondary.active, Some(duplicate));
        assert_eq!(workspace.focused_pane, Pane::Secondary);
    }

    #[test]
    fn keyboard_cycle_wraps_within_the_focused_pane() {
        let mut workspace = Workspace::default();
        let first = workspace.opened(None);
        let second = workspace.open_query(SearchDraft::default());
        let third = workspace.open_query(SearchDraft::default());
        workspace.activate(first);
        assert_eq!(workspace.adjacent_tab(false), Some(second));
        assert_eq!(workspace.adjacent_tab(true), Some(third));
        workspace.move_tab(third, Pane::Secondary, 0);
        assert_eq!(workspace.focused_active_tab(), Some(third));
        assert_eq!(workspace.adjacent_tab(false), Some(third));
        workspace.activate(second);
        assert_eq!(workspace.adjacent_tab(false), Some(first));
    }

    #[test]
    fn blank_queries_get_distinct_stable_titles() {
        let mut workspace = Workspace::default();
        workspace.opened(Some("files".into()));
        let first = workspace.open_query(SearchDraft::default());
        let second = workspace.open_query(SearchDraft::default());
        assert_eq!(workspace.tab(first).unwrap().title, "Query 1");
        assert_eq!(workspace.tab(second).unwrap().title, "Query 2");
        workspace.activate(first);
        workspace.update_current_search(SearchDraft {
            full_text: "orange".into(),
            ..Default::default()
        });
        assert!(workspace.tab(first).unwrap().title.contains("orange"));
        workspace.update_current_search(SearchDraft::default());
        assert_eq!(workspace.tab(first).unwrap().title, "Query 1");
    }

    #[test]
    fn closing_last_tab_leaves_an_empty_workspace() {
        let mut workspace = Workspace::default();
        let table = workspace.opened(Some("files".into()));
        assert!(workspace.close(table));
        assert!(workspace.primary.tabs.is_empty());
        assert_eq!(workspace.primary.active, None);
        assert_eq!(workspace.current_search, None);
        let query = workspace.open_query_in(SearchDraft::default(), Pane::Primary);
        assert_eq!(workspace.primary.active, Some(query));
    }

    #[test]
    fn routine_record_updates_do_not_steal_a_query_tab() {
        let mut workspace = Workspace::default();
        workspace.opened(None);
        let record = RecordTabSummary {
            id: 7,
            title: "item 7".into(),
            identity: "items / 7".into(),
            position: 1,
            can_back: false,
            can_previous: false,
            can_next: false,
            loading: false,
            links_loading: false,
            freshness: Default::default(),
        };
        workspace.sync_records(&[record.clone()], Some(7));
        let query = workspace.open_query(SearchDraft::default());
        workspace.move_tab(query, Pane::Secondary, 0);
        assert_eq!(workspace.secondary.active, Some(query));
        workspace.sync_records(&[record], Some(7));
        assert_eq!(workspace.secondary.active, Some(query));
    }
}
