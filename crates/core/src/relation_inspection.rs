//! Resident relation pages and the declared collection graph.

use std::sync::Arc;

use crate::inventory::{ActiveRecords, DiagnosticRef, FileStatus, RecordClass};
use crate::lifecycle::{IndexRevision, PublishedSegment};
use crate::lifecycle_catalog::FileCatalog;
use crate::model::{CollectionId, FileId, RecordSlot};
use crate::relation::{RelationCardinality, RelationId, RelationTarget};
use crate::relation_query::{MergedRecordSlot, MergedRelationView};
use crate::repository::{CollectionState, Repository};

/// Maximum number of outgoing occurrences or distinct backlink sources per page.
pub const MAX_RELATION_PAGE_SIZE: usize = 1024;


/// Stable record identity and resident provenance returned by inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectedRecord {
    pub collection: CollectionId,
    pub generation: u64,
    /// Qualifies a local slot when this record belongs to an indexed delta.
    pub segment_ordinal: Option<u64>,
    pub slot: RecordSlot,
    pub id: String,
    pub file: FileId,
    pub file_status: FileStatus,
    pub class: RecordClass,
    pub diagnostic_ref: DiagnosticRef,
}


/// Availability of one source relation field without reading its logical payload.
/// `Present` includes explicit null and empty arrays; the page reports those separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelationPresence {
    Missing,
    Present,
    Unavailable,
}


/// One source occurrence, including duplicate IDs at different array positions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutgoingReference {
    pub positions: Vec<u32>,
    pub target_id: String,
    pub resolved: Option<InspectedRecord>,
}


/// Bounded page of source occurrences in immutable adjacency order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutgoingReferencePage {
    pub relation: RelationId,
    pub source: InspectedRecord,
    pub target_collection: CollectionId,
    pub presence: RelationPresence,
    pub contains_null: bool,
    pub contains_empty_array: bool,
    pub total_occurrences: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub references: Vec<OutgoingReference>,
}


/// Bounded page of distinct sources from an existing typed reverse posting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BacklinkPage {
    pub relation: RelationId,
    pub target: InspectedRecord,
    pub source_collection: CollectionId,
    pub total_sources: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub sources: Vec<InspectedRecord>,
}


/// One configured collection, including collections that currently have no records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionGraphNode {
    pub id: CollectionId,
    pub name: String,
    pub generation: u64,
    pub record_count: usize,
    pub outgoing: Vec<RelationId>,
    pub incoming: Vec<RelationId>,
}


/// One declared relation; parallel source-target relations remain distinct.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionGraphEdge {
    pub id: RelationId,
    pub source: CollectionId,
    pub path: Vec<String>,
    pub target: CollectionId,
    pub cardinality: RelationCardinality,
    pub reverse_alias: Option<String>,
    pub occurrence_count: usize,
    pub unresolved_occurrence_count: u64,
}


/// Strongly connected collection IDs; a self-loop is cyclic on its own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionComponent {
    pub members: Vec<CollectionId>,
    pub cyclic: bool,
}


/// Resident declaration graph with adjacency and bounded-size cycle summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionGraph {
    pub nodes: Vec<CollectionGraphNode>,
    pub edges: Vec<CollectionGraphEdge>,
    pub components: Vec<CollectionComponent>,
}


fn page_limit(limit: usize) -> Result<usize, String> {
    if !(1..=MAX_RELATION_PAGE_SIZE).contains(&limit) {
        return Err(format!(
            "relation page limit must be between 1 and {MAX_RELATION_PAGE_SIZE}"
        ));
    }
    Ok(limit)
}

fn page_bounds(total: usize, offset: usize, limit: usize) -> (usize, usize, Option<usize>) {
    let start = offset.min(total);
    let end = start.saturating_add(limit).min(total);
    (start, end, (end < total).then_some(end))
}

fn inspected_record(
    repository: &Repository,
    collection: &CollectionState,
    slot: RecordSlot,
) -> Result<InspectedRecord, String> {
    let record = collection
        .generation
        .records
        .get(slot.0 as usize)
        .ok_or("relation record slot out of bounds")?;
    if record.slot != slot {
        return Err("relation record slot mismatch".into());
    }
    let file = repository
        .files
        .files
        .get(record.file.0 as usize)
        .ok_or("relation source file missing")?;
    Ok(InspectedRecord {
        collection: collection.generation.collection,
        generation: collection.generation.generation,
        segment_ordinal: None,
        slot,
        id: record.id.0.clone(),
        file: record.file,
        file_status: file.status,
        class: record.class,
        diagnostic_ref: record.diagnostic_ref,
    })
}


impl Repository {
    /// Page one record's outgoing occurrences without opening source files.
    pub fn relation_outgoing_page(
        &self,
        source_collection: &str,
        relation_path: &[String],
        source_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<OutgoingReferencePage, String> {
        self.relation_outgoing_page_with_active(
            source_collection,
            relation_path,
            source_id,
            offset,
            limit,
            None,
        )
    }

    pub(crate) fn relation_outgoing_page_for_view(
        &self,
        source_collection: &str,
        relation_path: &[String],
        source_id: &str,
        offset: usize,
        limit: usize,
        active: &[ActiveRecords],
    ) -> Result<OutgoingReferencePage, String> {
        self.relation_outgoing_page_with_active(
            source_collection,
            relation_path,
            source_id,
            offset,
            limit,
            Some(active),
        )
    }

    fn relation_outgoing_page_with_active(
        &self,
        source_collection: &str,
        relation_path: &[String],
        source_id: &str,
        offset: usize,
        limit: usize,
        active: Option<&[ActiveRecords]>,
    ) -> Result<OutgoingReferencePage, String> {
        let limit = page_limit(limit)?;
        let source = self
            .collections
            .iter()
            .find(|item| item.definition.name == source_collection)
            .ok_or("source collection not found")?;
        let relation = self
            .relation_index(source_collection, relation_path)
            .ok_or("relation not found in source collection")?;
        let slot = *source
            .id_lookup
            .get(source_id)
            .ok_or("source ID not found")?;
        if active
            .is_some_and(|domains| !domains[source.generation.collection.0 as usize].contains(slot))
        {
            return Err("source ID not found".into());
        }
        let target = self
            .collections
            .get(relation.target_collection.0 as usize)
            .ok_or("relation target collection missing")?;
        let edges = relation.edges(slot).ok_or("source relation slot missing")?;
        let (start, end, next_offset) = page_bounds(edges.len(), offset, limit);
        let mut references = Vec::with_capacity(end - start);
        for edge in &edges[start..end] {
            let target_slot = if let Some(domains) = active {
                target
                    .id_lookup
                    .get(edge.target_id())
                    .copied()
                    .filter(|slot| domains[relation.target_collection.0 as usize].contains(*slot))
            } else {
                match &edge.target {
                    RelationTarget::Resolved(slot) => Some(*slot),
                    RelationTarget::Unresolved(_) => None,
                }
            };
            references.push(OutgoingReference {
                positions: edge.positions.clone(),
                target_id: edge.target_id().to_owned(),
                resolved: target_slot
                    .map(|slot| inspected_record(self, target, slot))
                    .transpose()?,
            });
        }
        let presence = if relation.unavailable().contains(slot.0) {
            RelationPresence::Unavailable
        } else if relation.present().contains(slot.0) {
            RelationPresence::Present
        } else {
            RelationPresence::Missing
        };
        Ok(OutgoingReferencePage {
            relation: relation.id,
            source: inspected_record(self, source, slot)?,
            target_collection: relation.target_collection,
            presence,
            contains_null: relation.nulls().contains(slot.0),
            contains_empty_array: relation.empty_arrays().contains(slot.0),
            total_occurrences: edges.len(),
            offset,
            next_offset,
            references,
        })
    }

    /// Page distinct backlink sources through the relation's existing posting.
    pub fn relation_backlink_page(
        &self,
        target_collection: &str,
        reverse_alias: &str,
        target_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<BacklinkPage, String> {
        self.relation_backlink_page_with_active(
            target_collection,
            reverse_alias,
            target_id,
            offset,
            limit,
            None,
        )
    }

    pub(crate) fn relation_backlink_page_for_view(
        &self,
        target_collection: &str,
        reverse_alias: &str,
        target_id: &str,
        offset: usize,
        limit: usize,
        active: &[ActiveRecords],
    ) -> Result<BacklinkPage, String> {
        self.relation_backlink_page_with_active(
            target_collection,
            reverse_alias,
            target_id,
            offset,
            limit,
            Some(active),
        )
    }

    fn relation_backlink_page_with_active(
        &self,
        target_collection: &str,
        reverse_alias: &str,
        target_id: &str,
        offset: usize,
        limit: usize,
        active: Option<&[ActiveRecords]>,
    ) -> Result<BacklinkPage, String> {
        let limit = page_limit(limit)?;
        let target = self
            .collections
            .iter()
            .find(|item| item.definition.name == target_collection)
            .ok_or("target collection not found")?;
        let relation = self
            .backlink_relation(target_collection, reverse_alias)
            .ok_or("reverse relation alias not found")?;
        let target_slot = *target
            .id_lookup
            .get(target_id)
            .ok_or("target ID not found")?;
        if active.is_some_and(|domains| {
            !domains[target.generation.collection.0 as usize].contains(target_slot)
        }) {
            return Err("target ID not found".into());
        }
        let source = self
            .collections
            .get(relation.id.source.0 as usize)
            .ok_or("relation source collection missing")?;
        let posting = self.relation_reverse_sources(relation, target_id);
        let visible = posting.map(|slots| {
            active.map_or_else(
                || slots.clone(),
                |domains| slots & &domains[relation.id.source.0 as usize].bitmap(),
            )
        });
        let total = visible.as_ref().map_or(0, |slots| slots.len() as usize);
        let (start, end, next_offset) = page_bounds(total, offset, limit);
        let mut sources = Vec::with_capacity(end - start);
        if let Some(slots) = visible {
            for slot in slots.iter().skip(start).take(end - start) {
                sources.push(inspected_record(self, source, RecordSlot(slot))?);
            }
        }
        Ok(BacklinkPage {
            relation: relation.id,
            target: inspected_record(self, target, target_slot)?,
            source_collection: relation.id.source,
            total_sources: total,
            offset,
            next_offset,
            sources,
        })
    }

    /// Inspect all declared collections and relation edges without source reads.
    pub fn collection_graph(&self) -> Result<CollectionGraph, String> {
        self.collection_graph_with_active(None)
    }

    pub(crate) fn collection_graph_for_view(
        &self,
        active: &[ActiveRecords],
    ) -> Result<CollectionGraph, String> {
        self.collection_graph_with_active(Some(active))
    }

    fn collection_graph_with_active(
        &self,
        active: Option<&[ActiveRecords]>,
    ) -> Result<CollectionGraph, String> {
        let mut nodes = self
            .collections
            .iter()
            .map(|collection| CollectionGraphNode {
                id: collection.generation.collection,
                name: collection.definition.name.clone(),
                generation: collection.generation.generation,
                record_count: active.map_or(collection.generation.records.len(), |domains| {
                    domains[collection.generation.collection.0 as usize].len() as usize
                }),
                outgoing: Vec::new(),
                incoming: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut edges = Vec::new();
        for collection in &self.collections {
            for relation in &collection.relations {
                let source = relation.id.source.0 as usize;
                let target = relation.target_collection.0 as usize;
                nodes
                    .get_mut(source)
                    .ok_or("relation graph source collection missing")?
                    .outgoing
                    .push(relation.id);
                nodes
                    .get_mut(target)
                    .ok_or("relation graph target collection missing")?
                    .incoming
                    .push(relation.id);
                let (occurrence_count, unresolved_occurrence_count) = if let Some(domains) = active
                {
                    let mut occurrences = 0usize;
                    let mut unresolved = 0u64;
                    let target_collection = &self.collections[target];
                    for slot in domains[source].bitmap().iter() {
                        let edges = relation
                            .edges(RecordSlot(slot))
                            .ok_or("relation source slot missing")?;
                        occurrences = occurrences
                            .checked_add(edges.len())
                            .ok_or("relation occurrence count overflow")?;
                        for edge in edges {
                            if !target_collection
                                .id_lookup
                                .get(edge.target_id())
                                .is_some_and(|slot| domains[target].contains(*slot))
                            {
                                unresolved = unresolved
                                    .checked_add(1)
                                    .ok_or("relation unresolved count overflow")?;
                            }
                        }
                    }
                    (occurrences, unresolved)
                } else {
                    (relation.edge_count(), relation.unresolved_count())
                };
                edges.push(CollectionGraphEdge {
                    id: relation.id,
                    source: relation.id.source,
                    path: relation.declaration.path.clone(),
                    target: relation.target_collection,
                    cardinality: relation.declaration.cardinality,
                    reverse_alias: relation.declaration.reverse_alias.clone(),
                    occurrence_count,
                    unresolved_occurrence_count,
                });
            }
        }
        let components = graph_components(nodes.len(), &edges);
        Ok(CollectionGraph {
            nodes,
            edges,
            components,
        })
    }
}

fn inspected_merged_record(
    view: &MergedRelationView<'_>,
    address: MergedRecordSlot,
    revision: IndexRevision,
) -> Result<InspectedRecord, String> {
    let current = view.record(address)?.ok_or("inspection record retired")?;
    Ok(InspectedRecord {
        collection: address.collection,
        generation: revision.0,
        segment_ordinal: view.segment_ordinal(address),
        slot: address.slot,
        id: current.record.id.0.clone(),
        file: current.record.file,
        file_status: current.file.status,
        class: current.record.class,
        diagnostic_ref: current.record.diagnostic_ref,
    })
}

pub(crate) fn relation_outgoing_page_segmented(
    repo: &Repository,
    source_collection: &str,
    relation_path: &[String],
    source_id: &str,
    offset: usize,
    limit: usize,
    revision: IndexRevision,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
) -> Result<OutgoingReferencePage, String> {
    let limit = page_limit(limit)?;
    let source = repo.collections.iter()
        .find(|collection| collection.definition.name == source_collection)
        .ok_or("source collection not found")?;
    let relation = repo.relation_index(source_collection, relation_path)
        .ok_or("relation not found in source collection")?;
    let view = MergedRelationView::new(repo, base_active, segments, catalog)?;
    let source_address = view.resolve_id(source.generation.collection, source_id)?
        .ok_or("source ID not found")?;
    let (edges, present, null, empty, unavailable) =
        view.forward_edges(source_address, relation.id.ordinal as usize)?;
    let (start, end, next_offset) = page_bounds(edges.len(), offset, limit);
    let mut references = Vec::with_capacity(end - start);
    for (target_id, positions) in &edges[start..end] {
        let resolved = view.resolve_id(relation.target_collection, target_id)?
            .map(|address| inspected_merged_record(&view, address, revision))
            .transpose()?;
        references.push(OutgoingReference {
            positions: positions.to_vec(),
            target_id: (*target_id).to_owned(),
            resolved,
        });
    }
    Ok(OutgoingReferencePage {
        relation: relation.id,
        source: inspected_merged_record(&view, source_address, revision)?,
        target_collection: relation.target_collection,
        presence: if unavailable {
            RelationPresence::Unavailable
        } else if present {
            RelationPresence::Present
        } else {
            RelationPresence::Missing
        },
        contains_null: null,
        contains_empty_array: empty,
        total_occurrences: edges.len(),
        offset,
        next_offset,
        references,
    })
}

pub(crate) fn relation_backlink_page_segmented(
    repo: &Repository,
    target_collection: &str,
    reverse_alias: &str,
    target_id: &str,
    offset: usize,
    limit: usize,
    revision: IndexRevision,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
) -> Result<BacklinkPage, String> {
    let limit = page_limit(limit)?;
    let target = repo.collections.iter()
        .find(|collection| collection.definition.name == target_collection)
        .ok_or("target collection not found")?;
    let relation = repo.backlink_relation(target_collection, reverse_alias)
        .ok_or("reverse relation alias not found")?;
    let view = MergedRelationView::new(repo, base_active, segments, catalog)?;
    let target_address = view.resolve_id(target.generation.collection, target_id)?
        .ok_or("target ID not found")?;
    let sources = view.backlink_sources(relation.id, target_id)?;
    let (start, end, next_offset) = page_bounds(sources.len(), offset, limit);
    let page = sources[start..end].iter()
        .map(|address| inspected_merged_record(&view, *address, revision))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BacklinkPage {
        relation: relation.id,
        target: inspected_merged_record(&view, target_address, revision)?,
        source_collection: relation.id.source,
        total_sources: sources.len(),
        offset,
        next_offset,
        sources: page,
    })
}

pub(crate) fn collection_graph_segmented(
    repo: &Repository,
    revision: IndexRevision,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
) -> Result<CollectionGraph, String> {
    let view = MergedRelationView::new(repo, base_active, segments, catalog)?;
    let mut nodes = repo.collections.iter().enumerate().map(|(index, collection)| {
        let records = segments[index].iter()
            .fold(base_active[index].len() as usize, |count, segment| {
                count.saturating_add(segment.active.len() as usize)
            });
        CollectionGraphNode {
            id: collection.generation.collection,
            name: collection.definition.name.clone(),
            generation: revision.0,
            record_count: records,
            outgoing: Vec::new(),
            incoming: Vec::new(),
        }
    }).collect::<Vec<_>>();
    let mut edges = Vec::new();
    for (source_index, collection) in repo.collections.iter().enumerate() {
        for relation in &collection.relations {
            let target_index = relation.target_collection.0 as usize;
            nodes[source_index].outgoing.push(relation.id);
            nodes[target_index].incoming.push(relation.id);
            let mut occurrences = 0usize;
            let mut unresolved = 0u64;
            let mut visit = |address| -> Result<(), String> {
                view.record(address)?.ok_or("active graph source retired")?;
                let (references, _, _, _, _) =
                    view.forward_edges(address, relation.id.ordinal as usize)?;
                occurrences = occurrences.checked_add(references.len())
                    .ok_or("relation occurrence count overflow")?;
                for (target_id, _) in references {
                    if view.resolve_id(relation.target_collection, target_id)?.is_none() {
                        unresolved = unresolved.checked_add(1)
                            .ok_or("relation unresolved count overflow")?;
                    }
                }
                Ok(())
            };
            for slot in base_active[source_index].bitmap().iter() {
                visit(MergedRecordSlot {
                    collection: relation.id.source,
                    segment: None,
                    slot: RecordSlot(slot),
                })?;
            }
            for (index, segment) in segments[source_index].iter().enumerate() {
                for slot in segment.active.bitmap().iter() {
                    visit(MergedRecordSlot {
                        collection: relation.id.source,
                        segment: Some(index),
                        slot: RecordSlot(slot),
                    })?;
                }
            }
            edges.push(CollectionGraphEdge {
                id: relation.id,
                source: relation.id.source,
                path: relation.declaration.path.clone(),
                target: relation.target_collection,
                cardinality: relation.declaration.cardinality,
                reverse_alias: relation.declaration.reverse_alias.clone(),
                occurrence_count: occurrences,
                unresolved_occurrence_count: unresolved,
            });
        }
    }
    let components = graph_components(nodes.len(), &edges);
    Ok(CollectionGraph { nodes, edges, components })
}


fn graph_components(node_count: usize, edges: &[CollectionGraphEdge]) -> Vec<CollectionComponent> {
    let mut forward = vec![Vec::new(); node_count];
    let mut reverse = vec![Vec::new(); node_count];
    for edge in edges {
        let source = edge.source.0 as usize;
        let target = edge.target.0 as usize;
        forward[source].push(target);
        reverse[target].push(source);
    }
    let mut seen = vec![false; node_count];
    let mut finished = Vec::with_capacity(node_count);
    for root in 0..node_count {
        if seen[root] {
            continue;
        }
        seen[root] = true;
        let mut stack = vec![(root, 0usize)];
        while let Some((node, next)) = stack.last_mut() {
            if let Some(&target) = forward[*node].get(*next) {
                *next += 1;
                if !seen[target] {
                    seen[target] = true;
                    stack.push((target, 0));
                }
            } else {
                finished.push(*node);
                stack.pop();
            }
        }
    }
    seen.fill(false);
    let mut components = Vec::new();
    for &root in finished.iter().rev() {
        if seen[root] {
            continue;
        }
        seen[root] = true;
        let mut members = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            members.push(CollectionId(node as u32));
            for &target in &reverse[node] {
                if !seen[target] {
                    seen[target] = true;
                    stack.push(target);
                }
            }
        }
        members.sort_unstable();
        let cyclic = members.len() > 1 || forward[root].contains(&root);
        components.push(CollectionComponent { members, cyclic });
    }
    components.sort_by_key(|component| component.members[0]);
    components
}

#[cfg(test)]
#[path = "relation_inspection.test.rs"]
mod tests;
