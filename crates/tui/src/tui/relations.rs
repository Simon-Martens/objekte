//! Resident relation rows for the detail view.

use objekte_search_core::relation_inspection::{MAX_RELATION_PAGE_SIZE, RelationPresence};
use objekte_search_core::repository::{Repository, SearchHit};

const PAGE_SIZE: usize = 100;


#[derive(Clone, Debug)]
pub(super) enum LinkAction {
    Target {
        collection: String,
        id: String,
    },
    MoreOutgoing {
        source: String,
        path: Vec<String>,
        id: String,
        offset: usize,
    },
    MoreBacklinks {
        target: String,
        alias: String,
        id: String,
        offset: usize,
    },
}


#[derive(Clone, Debug)]
pub(super) struct RelationRow {
    pub(super) label: String,
    pub(super) value: String,
    pub(super) action: Option<LinkAction>,
}


impl RelationRow {
    fn plain(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            action: None,
        }
    }

    fn target(label: String, value: String, collection: String, id: String) -> Self {
        Self {
            label,
            value,
            action: Some(LinkAction::Target { collection, id }),
        }
    }
}


fn collection_name(repo: &Repository, id: objekte_search_core::model::CollectionId) -> String {
    repo.collections().get(id.0 as usize).map_or_else(
        || format!("collection {}", id.0),
        |collection| collection.definition.name.clone(),
    )
}

pub(super) fn rows(repo: &Repository, hit: &SearchHit) -> Vec<RelationRow> {
    let graph = match repo.collection_graph() {
        Ok(graph) => graph,
        Err(error) => return vec![RelationRow::plain("Relation inspection error", error)],
    };
    let mut rows = Vec::new();
    for edge in &graph.edges {
        if collection_name(repo, edge.source) == hit.collection {
            let target = collection_name(repo, edge.target);
            let label = format!("{} → {target}", edge.path.join("."));
            rows.push(RelationRow::plain("Outgoing", label));
            rows.extend(outgoing(
                repo,
                &hit.collection,
                &edge.path,
                &hit.record_id,
                0,
            ));
        }
        if collection_name(repo, edge.target) == hit.collection
            && let Some(alias) = &edge.reverse_alias
        {
            let source = collection_name(repo, edge.source);
            rows.push(RelationRow::plain(
                "Backlinks",
                format!("{alias} ← {source}"),
            ));
            rows.extend(backlinks(repo, &hit.collection, alias, &hit.record_id, 0));
        }
    }
    rows
}

fn outgoing(
    repo: &Repository,
    source: &str,
    path: &[String],
    id: &str,
    offset: usize,
) -> Vec<RelationRow> {
    let page = match repo.relation_outgoing_page(
        source,
        path,
        id,
        offset,
        PAGE_SIZE.min(MAX_RELATION_PAGE_SIZE),
    ) {
        Ok(page) => page,
        Err(error) => return vec![RelationRow::plain("  Inspection error", error)],
    };
    let mut rows = Vec::new();
    if offset == 0 {
        match page.presence {
            RelationPresence::Missing => {
                rows.push(RelationRow::plain("  Missing", "field is absent"))
            }
            RelationPresence::Unavailable => rows.push(RelationRow::plain(
                "  Unavailable",
                "logical field unavailable",
            )),
            RelationPresence::Present => {
                if page.contains_null {
                    rows.push(RelationRow::plain("  Null", "explicit null"));
                }
                if page.contains_empty_array {
                    rows.push(RelationRow::plain("  Empty", "empty array"));
                }
                if page.total_occurrences == 0 && !page.contains_null && !page.contains_empty_array
                {
                    rows.push(RelationRow::plain("  Empty", "no references"));
                }
            }
        }
    }
    let target = collection_name(repo, page.target_collection);
    for reference in page.references {
        let position = if reference.positions.is_empty() {
            String::new()
        } else {
            format!("{:?} ", reference.positions)
        };
        if let Some(record) = reference.resolved {
            rows.push(RelationRow::target(
                format!("  {position}{}", reference.target_id),
                format!("{target} · {}", record.file_status.label()),
                target.clone(),
                reference.target_id,
            ));
        } else {
            rows.push(RelationRow::plain(
                format!("  {position}{}", reference.target_id),
                format!("dangling · {target} target missing"),
            ));
        }
    }
    if let Some(next) = page.next_offset {
        rows.push(RelationRow {
            label: "  More outgoing…".into(),
            value: format!("{next}/{} shown", page.total_occurrences),
            action: Some(LinkAction::MoreOutgoing {
                source: source.into(),
                path: path.to_vec(),
                id: id.into(),
                offset: next,
            }),
        });
    }
    rows
}

fn backlinks(
    repo: &Repository,
    target: &str,
    alias: &str,
    id: &str,
    offset: usize,
) -> Vec<RelationRow> {
    let page = match repo.relation_backlink_page(
        target,
        alias,
        id,
        offset,
        PAGE_SIZE.min(MAX_RELATION_PAGE_SIZE),
    ) {
        Ok(page) => page,
        Err(error) => return vec![RelationRow::plain("  Inspection error", error)],
    };
    let source = collection_name(repo, page.source_collection);
    let mut rows = Vec::new();
    if offset == 0 && page.total_sources == 0 {
        rows.push(RelationRow::plain("  None", "no backlinks"));
    }
    for record in page.sources {
        rows.push(RelationRow::target(
            format!("  {}", record.id),
            format!("{source} · {}", record.file_status.label()),
            source.clone(),
            record.id,
        ));
    }
    if let Some(next) = page.next_offset {
        rows.push(RelationRow {
            label: "  More backlinks…".into(),
            value: format!("{next}/{} shown", page.total_sources),
            action: Some(LinkAction::MoreBacklinks {
                target: target.into(),
                alias: alias.into(),
                id: id.into(),
                offset: next,
            }),
        });
    }
    rows
}

pub(super) fn continuation(repo: &Repository, action: &LinkAction) -> Vec<RelationRow> {
    match action {
        LinkAction::Target { .. } => Vec::new(),
        LinkAction::MoreOutgoing {
            source,
            path,
            id,
            offset,
        } => outgoing(repo, source, path, id, *offset),
        LinkAction::MoreBacklinks {
            target,
            alias,
            id,
            offset,
        } => backlinks(repo, target, alias, id, *offset),
    }
}
