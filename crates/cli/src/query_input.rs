//! Sequential query input and translation into normal core requests.

use std::fs::File;
use std::io::{self, BufRead, BufReader};

use objekte_search_core::query::{
    CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_structured_filter,
};
use objekte_search_core::workload::ReturnMode;

use crate::arguments::Arguments;


/// Query spelling, source location, and explicit input kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryInput {
    pub original: String,
    pub line: Option<usize>,
    pub kind: QueryKind,
}


/// How one input is parsed before the core request is built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryKind {
    Structured,
    Fts,
}


/// One direct query or a buffered physical-line reader.
pub struct QuerySource {
    direct: Option<QueryInput>,
    reader: Option<Box<dyn BufRead>>,
    line: usize,
    first_line: bool,
    yielded: usize,
}


impl QueryInput {
    pub fn parse(&self, args: &Arguments) -> Result<(Query, Option<String>), String> {
        let (fts, filter, sort) = match self.kind {
            QueryKind::Fts => {
                let literal = if args.fts.is_some() && self.line.is_none() {
                    self.original.clone()
                } else {
                    serde_json::from_str::<String>(&self.original)
                        .map_err(|error| format!("invalid FTS JSON string literal: {error}"))?
                };
                let fts = FtsQuery {
                    case_sensitivity: CaseSensitivity::Sensitive,
                    mode: FtsMode::Literal,
                    literal: literal.clone(),
                    pattern_parameter: None,
                    fields: Vec::new(),
                };
                (Some((fts, literal)), None, Vec::new())
            }
            QueryKind::Structured => {
                let parsed = parse_structured_filter(&self.original)?;
                (None, parsed.filter, parsed.sort.unwrap_or_default())
            }
        };
        let literal = fts.as_ref().map(|(_, literal)| literal.clone());
        Ok((
            Query {
                collections: if args.multiple_collections() {
                    Vec::new()
                } else {
                    vec!["files".into()]
                },
                fts: fts.map(|(query, _)| query),
                filter,
                options: QueryOptions {
                    sort,
                    limit: args.limit.unwrap_or(0),
                    offset: args.offset,
                    count: args.count,
                    return_mode: ReturnMode::Bytes,
                    evidence: true,
                },
            },
            literal,
        ))
    }
}


impl QuerySource {
    pub fn open(args: &Arguments) -> Result<Self, String> {
        let direct = if let Some(text) = &args.fts {
            Some(QueryInput {
                original: text.clone(),
                line: None,
                kind: QueryKind::Fts,
            })
        } else {
            args.query.as_ref().map(|text| QueryInput {
                original: text.clone(),
                line: None,
                kind: kind_of(text),
            })
        };
        let reader: Option<Box<dyn BufRead>> = match args.query_file.as_deref() {
            Some("-") => Some(Box::new(BufReader::new(io::stdin()))),
            Some(path) => Some(Box::new(BufReader::new(
                File::open(path).map_err(|error| format!("{path}: {error}"))?,
            ))),
            None => None,
        };
        Ok(Self {
            direct,
            reader,
            line: 0,
            first_line: true,
            yielded: 0,
        })
    }

    pub fn next_input(&mut self) -> Result<Option<QueryInput>, String> {
        if let Some(input) = self.direct.take() {
            self.yielded += 1;
            return Ok(Some(input));
        }
        let Some(reader) = &mut self.reader else {
            return Ok(None);
        };
        loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).map_err(|error| {
                format!("query file line {}: {error}", self.line.saturating_add(1))
            })?;
            if read == 0 {
                return Ok(None);
            }
            self.line += 1;
            if self.first_line {
                self.first_line = false;
                if let Some(rest) = line.strip_prefix('\u{feff}') {
                    line = rest.to_owned();
                }
            }
            let original = line.trim().to_owned();
            if original.is_empty() {
                continue;
            }
            self.yielded += 1;
            return Ok(Some(QueryInput {
                kind: kind_of(&original),
                original,
                line: Some(self.line),
            }));
        }
    }

    pub fn is_file(&self) -> bool {
        self.reader.is_some()
    }

    pub fn yielded(&self) -> usize {
        self.yielded
    }
}


fn kind_of(text: &str) -> QueryKind {
    if text.starts_with('"') {
        QueryKind::Fts
    } else {
        QueryKind::Structured
    }
}

#[cfg(test)]
#[path = "query_input.test.rs"]
mod tests;
