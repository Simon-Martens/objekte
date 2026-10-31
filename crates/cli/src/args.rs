//! Command-line syntax.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const HELP: &str = "objekte — read-only file repository library driver

Usage:
  objekte REPOSITORY view COLLECTION [--sort FIELD] [--direction asc|desc]
      [--then-sort FIELD:asc|desc]
      [--filter FIELD=VALUE] [--columns a,b] [--limit N] [--offset N] [--count]
  objekte REPOSITORY get COLLECTION ID
  objekte REPOSITORY validate [COLLECTION]
  objekte REPOSITORY query [COLLECTION] FILTER [--param NAME=JSON]
      [--limit N] [--offset N] [--count] [--backend auto|direct|grep|bigram]
  objekte REPOSITORY search [COLLECTION] PATTERN [--mode word|regex|raw-regex]
      [--field PATH] [--case-sensitive] [--limit N] [--offset N] [--count]
  objekte REPOSITORY inspect [--collections] [--indexes]
  objekte REPOSITORY relations --collections
  objekte REPOSITORY relations --from COLLECTION/ID [--check-targets]
  objekte REPOSITORY relations --to COLLECTION/ID [--complete] [--check-targets]
  objekte REPOSITORY cache status
  objekte REPOSITORY cache refresh-config
  objekte REPOSITORY cache refresh --collection NAME
  objekte REPOSITORY cache warm [--collection NAME]
  objekte REPOSITORY cache rebuild [--collection NAME]
  objekte REPOSITORY cache warm-text [--collection NAME]
  objekte REPOSITORY cache rebuild-text [--collection NAME]
  objekte REPOSITORY cache validate --collection NAME

Options:
  --cache memory|disabled         Default: memory
  --reconcile metadata-on-open|metadata-on-request|hash-on-request
                                 Default: metadata-on-open
  --output table|json|ndjson|events
                                  Views default to table, other commands to json; events stream query/search hits
  --diagnostic, --diagnostics     Emit pretty JSON diagnostics with time and level to stderr
  --collection NAME               Query scope (alternative to positional COLLECTION); cache target
  --param NAME=JSON                Bind a typed query value (repeatable; quote JSON strings)
  --backend auto|direct|grep|bigram Query engine; auto uses the in-memory bigram index
  --mode word|regex|raw-regex     Full-text word or regex search; raw-regex sees markup
  --field PATH                    Limit full-text search to a JSON leaf path
  --case-sensitive                Match case exactly in full-text search
  --help, --version

Examples:
  objekte examples/musenalm view places
  objekte examples/musenalm view contents --sort numbering --limit 20 --diagnostics
  objekte examples/musenalm view contents --filter 'year>=1800' --output json
  objekte examples/musenalm query contents 'year >= 1800' --count
";


// INFO: Parsed positional arguments, options, flags, filters, and query parameters.
pub(super) struct Args {
    pub(super) pos: Vec<String>,
    pub(super) values: BTreeMap<String, String>,
    pub(super) flags: BTreeSet<String>,
    pub(super) filters: Vec<String>,
    pub(super) sub_sorts: Vec<String>,
    pub(super) parameters: BTreeMap<String, Value>,
}


pub(super) fn args() -> Result<Args, String> {
    let mut out = Args {
        pos: Vec::new(),
        values: BTreeMap::new(),
        flags: BTreeSet::new(),
        filters: Vec::new(),
        sub_sorts: Vec::new(),
        parameters: BTreeMap::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--cache" | "--reconcile" | "--output" | "--sort" | "--direction" | "--columns"
            | "--limit" | "--offset" | "--collection" | "--from" | "--to" | "--filter"
            | "--param" | "--then-sort" | "--backend" | "--mode" | "--field" => {
                let v = args
                    .next()
                    .filter(|s| !s.starts_with("--"))
                    .ok_or_else(|| format!("{a} requires a value"))?;
                if a == "--param" {
                    let (name, raw) = v.split_once('=').ok_or("--param requires NAME=JSON")?;
                    if name.is_empty() {
                        return Err("--param requires a name".into());
                    }
                    let value = serde_json::from_str(raw)
                        .map_err(|e| format!("invalid --param JSON: {e}"))?;
                    if out.parameters.insert(name.into(), value).is_some() {
                        return Err(format!("duplicate parameter {name}"));
                    }
                } else if a == "--filter" {
                    out.filters.push(v)
                } else if a == "--then-sort" {
                    out.sub_sorts.push(v)
                } else if out.values.insert(a.clone(), v).is_some() {
                    return Err(format!("duplicate option {a}"));
                }
            }
            "--collections" | "--indexes" | "--count" | "--complete" | "--check-targets"
            | "--case-sensitive" | "--diagnostics" | "--diagnostic" => {
                out.flags.insert(if a == "--diagnostic" {
                    "--diagnostics".into()
                } else {
                    a
                });
            }
            "--help" | "-h" => {
                print!("{HELP}");
                std::process::exit(0)
            }
            "--version" | "-V" => {
                println!("objekte {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0)
            }
            a if a.starts_with('-') => return Err(format!("unknown option {a}")),
            _ => out.pos.push(a),
        }
    }
    Ok(out)
}
