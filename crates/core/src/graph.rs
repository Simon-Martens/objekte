use crate::{Collection, Field, PathStep, Result, error::config, schema::escape};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};


// INFO: Declared path from a source collection to a target collection.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionRelation {
    pub source: String,
    pub path: Vec<PathStep>,
    pub target: String,
}


// INFO: Declared edges, connected components, and collection cycles.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CollectionGraph {
    pub edges: Vec<CollectionRelation>,
    pub components: Vec<Vec<String>>,
    pub cycles: Vec<Vec<String>>,
}


impl CollectionGraph {
    pub(crate) fn build(collections: &BTreeMap<String, Collection>) -> Result<Self> {
        let mut edges = Vec::new();
        fn walk(
            source: &str,
            field: &Field,
            path: &mut Vec<PathStep>,
            edges: &mut Vec<CollectionRelation>,
        ) {
            if let Some(target) = &field.relation {
                edges.push(CollectionRelation {
                    source: source.into(),
                    path: path.clone(),
                    target: target.clone(),
                })
            }
            for (name, f) in &field.properties {
                path.push(PathStep::Field(name.clone()));
                walk(source, f, path, edges);
                path.pop();
            }
            if let Some(f) = &field.items {
                path.push(PathStep::Each);
                walk(source, f, path, edges);
                path.pop();
            }
        }
        for (name, c) in collections {
            walk(name, &c.schema.field, &mut Vec::new(), &mut edges)
        }
        let names = collections.keys().cloned().collect::<Vec<_>>();
        let ids = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i))
            .collect::<HashMap<_, _>>();
        let mut adj = vec![Vec::new(); names.len()];
        for e in &edges {
            let Some(&target) = ids.get(&e.target) else {
                let pointer = e
                    .path
                    .iter()
                    .map(|step| match step {
                        PathStep::Field(name) => format!("/properties/{}", escape(name)),
                        PathStep::Each => "/items".into(),
                    })
                    .collect::<String>()
                    + "/x-relation/collection";
                return Err(config(
                    &collections[&e.source].schema_path,
                    pointer,
                    format!("unknown target collection {}", e.target),
                ));
            };
            adj[ids[&e.source]].push(target);
        }
        // Iterative Tarjan avoids making graph size a call-stack limit.
        let n = names.len();
        let mut index = vec![None; n];
        let mut low = vec![0; n];
        let mut active = vec![false; n];
        let mut stack = Vec::new();
        let mut clock = 0;
        let mut components = Vec::new();
        for start in 0..n {
            if index[start].is_some() {
                continue;
            }
            index[start] = Some(clock);
            low[start] = clock;
            clock += 1;
            stack.push(start);
            active[start] = true;
            let mut dfs = vec![(start, 0)];
            while let Some((v, next)) = dfs.last_mut() {
                let v = *v;
                if *next < adj[v].len() {
                    let w = adj[v][*next];
                    *next += 1;
                    if index[w].is_none() {
                        index[w] = Some(clock);
                        low[w] = clock;
                        clock += 1;
                        stack.push(w);
                        active[w] = true;
                        dfs.push((w, 0));
                    } else if active[w] {
                        low[v] = low[v].min(index[w].unwrap());
                    }
                } else {
                    dfs.pop();
                    if let Some(&(parent, _)) = dfs.last() {
                        low[parent] = low[parent].min(low[v]);
                    }
                    if low[v] == index[v].unwrap() {
                        let mut component = Vec::new();
                        loop {
                            let w = stack.pop().unwrap();
                            active[w] = false;
                            component.push(names[w].clone());
                            if w == v {
                                break;
                            }
                        }
                        component.sort();
                        components.push(component);
                    }
                }
            }
        }
        components.sort();
        let cycles = components
            .iter()
            .filter(|c| c.len() > 1 || edges.iter().any(|e| e.source == c[0] && e.target == c[0]))
            .cloned()
            .collect();
        Ok(Self {
            edges,
            components,
            cycles,
        })
    }
}
