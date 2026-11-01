//! Command-line syntax.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const HELP: &str = "objekte — read-only file repository library driver

Usage:
  objekte [REPOSITORY]                  Open the interactive TUI (default repository: .)
  objekte REPOSITORY tui                Open the interactive TUI explicitly
  objekte REPOSITORY view NAME [--arg NAME=JSON] [--query-arg QUERY.FIELD=JSON]
  objekte REPOSITORY get COLLECTION ID
  objekte REPOSITORY validate [COLLECTION]
  objekte REPOSITORY inferschema [COLLECTION] [--no-auto-index]
  objekte REPOSITORY writeschema [COLLECTION] [--no-auto-index]
  objekte REPOSITORY query [COLLECTION] 'FILTER SORT BY FIELD [ASC|DESC] [&& FIELD ...]' [--param NAME=JSON]
      [--limit N] [--offset N] [--count] [--backend auto|direct|grep|bigram]
  objekte REPOSITORY search [COLLECTION] 'PATTERN SORT BY FIELD [ASC|DESC] [&& FIELD ...]' [--mode word|raw-word|regex|raw-regex]
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
  objekte REPOSITORY cache save [--collection NAME]
  objekte REPOSITORY cache reindex [--collection NAME]
  objekte REPOSITORY cache warm-text [--collection NAME]
  objekte REPOSITORY cache rebuild-text [--collection NAME]
  objekte REPOSITORY cache validate --collection NAME

Options:
  --cache memory|disabled         Default: memory
  --reconcile metadata-on-open|metadata-on-request|hash-on-request
                                 Default: metadata-on-open
  --total-limit N                 Maximum accessible matches per query (default: 10000)
  --output table|json|ndjson|events
                                  Sorted events publish provisional page replacements
  --diagnostic, --diagnostics     Emit pretty JSON diagnostics with time and level to stderr
  --collection NAME               Query or inference scope (alternative to positional COLLECTION); cache target
  --no-auto-index                 Infer types without new ordered posting rules
  --param NAME=JSON                Bind a typed query value (repeatable; quote JSON strings)
  --arg NAME=JSON                  Bind a declared view argument
  --query-arg QUERY.FIELD=JSON     Override a native query request field in a view
  --backend auto|direct|grep|bigram Query engine; auto uses the in-memory bigram index
  --mode word|raw-word|regex|raw-regex
                                  Literal substring or regex search; raw modes see markup
  --field PATH                    Limit full-text search to a JSON leaf path
  --case-sensitive                Match case exactly in full-text search
                                  TUI: Tab changes mode; Ctrl+Tab changes collection;
                                  Ctrl+R/F/E change regex, text, and word-case options;
                                  Alt+S saves indexes; Alt+R reindexes and saves
  --help, --version

Examples:
  objekte examples/musenalm view contents_list --arg page=2
  objekte examples/musenalm query contents 'year >= 1800' --count
  objekte examples/musenalm query contents 'year >= 1800 SORT BY preferred_title' --count
  objekte examples/musenalm inferschema contents
  objekte examples/musenalm writeschema contents
";


// INFO: Parsed positional arguments, options, flags, filters, and query parameters.
pub(super) struct Args {
    pub(super) pos: Vec<String>,
    pub(super) values: BTreeMap<String, String>,
    pub(super) flags: BTreeSet<String>,
    pub(super) filters: Vec<String>,
    pub(super) sub_sorts: Vec<String>,
    pub(super) parameters: BTreeMap<String, Value>,
    pub(super) view_arguments: BTreeMap<String, Value>,
    pub(super) query_arguments: BTreeMap<String, BTreeMap<String, Value>>,
}


pub(super) fn args() -> Result<Args, String> {
    let mut out = Args {
        pos: Vec::new(),
        values: BTreeMap::new(),
        flags: BTreeSet::new(),
        filters: Vec::new(),
        sub_sorts: Vec::new(),
        parameters: BTreeMap::new(),
        view_arguments: BTreeMap::new(),
        query_arguments: BTreeMap::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--cache" | "--reconcile" | "--total-limit" | "--output" | "--sort" | "--direction" | "--columns"
            | "--limit" | "--offset" | "--collection" | "--from" | "--to" | "--filter"
            | "--param" | "--then-sort" | "--backend" | "--mode" | "--field" | "--arg"
            | "--query-arg" => {
                let v = args
                    .next()
                    .filter(|s| !s.starts_with("--"))
                    .ok_or_else(|| format!("{a} requires a value"))?;
                if matches!(a.as_str(), "--param" | "--arg" | "--query-arg") {
                    let (name, raw) = v.split_once('=').ok_or("argument requires NAME=JSON")?;
                    if name.is_empty() {
                        return Err("argument requires a name".into());
                    }
                    let value = serde_json::from_str(raw)
                        .map_err(|e| format!("invalid argument JSON: {e}"))?;
                    let target = match a.as_str() {
                        "--param" => &mut out.parameters,
                        "--arg" => &mut out.view_arguments,
                        _ => {
                            let (query, field) = name
                                .split_once('.')
                                .ok_or("--query-arg requires QUERY.FIELD=JSON")?;
                            if query.is_empty() || field.is_empty() {
                                return Err("--query-arg requires QUERY.FIELD=JSON".into());
                            }
                            if out
                                .query_arguments
                                .entry(query.into())
                                .or_default()
                                .insert(field.into(), value)
                                .is_some()
                            {
                                return Err(format!("duplicate query argument {name}"));
                            }
                            continue;
                        }
                    };
                    if target.insert(name.into(), value).is_some() {
                        return Err(format!("duplicate argument {name}"));
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
            | "--case-sensitive" | "--diagnostics" | "--diagnostic" | "--no-auto-index" => {
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
