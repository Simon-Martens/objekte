//! Command-line syntax and application data directory selection.

use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub(super) const HELP: &str = "objekte — read-only file repository library driver

Usage:
  objekte REPOSITORY view COLLECTION [--sort FIELD] [--direction asc|desc]
      [--then-sort FIELD:asc|desc]
      [--filter FIELD=VALUE] [--columns a,b] [--limit N] [--offset N] [--count]
  objekte REPOSITORY get COLLECTION ID
  objekte REPOSITORY validate [COLLECTION]
  objekte REPOSITORY query [COLLECTION] FILTER [--param NAME=JSON]
      [--limit N] [--offset N] [--count]
  objekte REPOSITORY inspect [--collections] [--indexes]
  objekte REPOSITORY relations --collections
  objekte REPOSITORY relations --from COLLECTION/ID [--check-targets]
  objekte REPOSITORY relations --to COLLECTION/ID [--complete] [--check-targets]
  objekte REPOSITORY cache status
  objekte REPOSITORY cache refresh-config
  objekte REPOSITORY cache refresh --collection NAME
  objekte REPOSITORY cache validate --collection NAME

Options:
  --data-dir PATH                 Application state directory
  --cache sqlite|disabled         Default: sqlite
  --sqlite-cache-mib N|default     SQLite page-cache budget per connection (default: 64 MiB)
  --ingest-workers N              Fresh-cache workers (default: CPU threads / 2, minimum 1)
  --reconcile metadata-on-open|metadata-on-request|hash-on-open|hash-on-request
                                 Default: metadata-on-open
  --namespace NAME                Reuse a named cache (default: default)
  --fresh-cache                   Use a new namespace
  --output table|json|ndjson       Views default to table, other commands to json; timings in ms
  --background-workers N          Validation workers (default: CPU threads / 4, minimum 1)
  --validation background|manual|wait  Validation policy (default: background)
  --diagnostics                   Emit timed SQL and counters to stderr; await background validation
  --collection NAME               Query scope (alternative to positional COLLECTION); cache target
  --param NAME=JSON                Bind a typed query value (repeatable; quote JSON strings)
  --help, --version

Examples:
  objekte examples/musenalm view places --data-dir /tmp/objekte-a
  objekte examples/musenalm view contents --sort numbering --limit 20 --diagnostics
  objekte examples/musenalm view contents --filter 'year>=1800' --output json
  objekte examples/musenalm query contents 'year >= 1800' --count
";
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
            "--data-dir"
            | "--cache"
            | "--reconcile"
            | "--namespace"
            | "--output"
            | "--sort"
            | "--direction"
            | "--columns"
            | "--limit"
            | "--offset"
            | "--collection"
            | "--from"
            | "--to"
            | "--filter"
            | "--param"
            | "--then-sort"
            | "--sqlite-cache-mib"
            | "--ingest-workers"
            | "--background-workers"
            | "--validation" => {
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
            | "--fresh-cache" | "--diagnostics" => {
                out.flags.insert(a);
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
pub(super) fn data_dir() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(p).join("objekte"));
    }
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("objekte"))
            .ok_or_else(|| "set --data-dir".into());
    }
    let home = std::env::var_os("HOME").ok_or_else(|| "set --data-dir".to_owned())?;
    Ok(PathBuf::from(home).join(if cfg!(target_os = "macos") {
        "Library/Application Support/objekte"
    } else {
        ".local/share/objekte"
    }))
}
