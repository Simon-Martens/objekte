//! Discover collection ownership from JSON folders and optional schema bases.

use super::*;


/// Discovery inputs retained by a repository handle for configuration reloads.
#[derive(Clone)]
pub(crate) struct Discovery {
    pub data_root: Option<PathBuf>,
    pub schemas: Vec<(PathBuf, PathBuf)>,
    pub use_default_schemas: bool,
}


impl Default for Discovery {
    fn default() -> Self {
        Self {
            data_root: None,
            schemas: Vec::new(),
            use_default_schemas: true,
        }
    }
}


/// One real directory and whether it owns JSON files directly.
struct Directory {
    path: PathBuf,
    direct_json: bool,
}


/// Collection identity and source mapping declared beside its JSON Schema.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionMetadata {
    name: String,
    path: String,
    r#type: String,
    format: String,
    id: String,
}


impl RepositoryModel {
    pub(crate) fn discover(root: &Path, settings: &Discovery) -> Result<Self> {
        let root = fs::canonicalize(root).map_err(|error| io(root, error))?;
        if settings.use_default_schemas
            && (root.join(".objekte/collections").is_dir()
                || root.join(".objekte/views").is_dir())
        {
            return Self::discover_objekte(&root, settings);
        }
        let data = match &settings.data_root {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => root.join(path),
            None if root.join("data").is_dir() => root.join("data"),
            None => root.clone(),
        };
        let data = fs::canonicalize(&data).map_err(|error| io(&data, error))?;
        if !data.starts_with(&root) || !data.is_dir() {
            return Err(config(
                &data,
                "",
                "data root must be a directory inside repository",
            ));
        }
        let mut directories = Vec::new();
        discover_directories(&data, &mut directories)?;

        let mut schema_bases = BTreeMap::new();
        if settings.use_default_schemas {
            for directory in &directories {
                let name = collection_name(&data, &directory.path)?;
                let local = directory.path.join(".editor/schema.json");
                let shared = root
                    .join(".editor/collections")
                    .join(&name)
                    .join("schema.json");
                if local.is_file() {
                    schema_bases.insert(directory.path.clone(), local);
                } else if shared.is_file() {
                    schema_bases.insert(directory.path.clone(), shared);
                }
            }
        }
        let mut explicit_bases = BTreeSet::new();
        for (base, schema) in &settings.schemas {
            let base = if base.is_absolute() {
                base.clone()
            } else {
                data.join(base)
            };
            let base = fs::canonicalize(&base).map_err(|error| io(&base, error))?;
            if !base.starts_with(&data) || !directories.iter().any(|entry| entry.path == base) {
                return Err(config(
                    &base,
                    "",
                    "schema base must be a discovered data directory",
                ));
            }
            if !explicit_bases.insert(base.clone()) {
                return Err(config(&base, "", "schema base folders overlap"));
            }
            let schema = if schema.is_absolute() {
                schema.clone()
            } else {
                root.join(schema)
            };
            let schema = fs::canonicalize(&schema).map_err(|error| io(&schema, error))?;
            if let Ok(relative) = schema.strip_prefix(&data)
                && !relative
                    .components()
                    .any(|component| component.as_os_str() == ".editor")
            {
                return Err(config(
                    &schema,
                    "",
                    "schema file must be outside the JSON data tree or inside .editor",
                ));
            }
            schema_bases.insert(base, schema);
        }
        let bases = schema_bases.keys().collect::<Vec<_>>();
        for (position, base) in bases.iter().enumerate() {
            if bases[position + 1..]
                .iter()
                .any(|other| other.starts_with(base))
            {
                return Err(config(base, "", "schema base folders overlap"));
            }
        }

        let mut files = BTreeMap::new();
        let mut input_collections = BTreeMap::<PathBuf, BTreeSet<String>>::new();
        let defaults_path = root.join(".editor/ui.json");
        let defaults = if defaults_path.is_file() {
            read::<RepoUi>(&defaults_path, &mut files)?.defaults
        } else {
            Defaults::default()
        };
        let default_page = defaults.page_size.unwrap_or(50);
        if default_page == 0 || default_page > i64::MAX as usize {
            return Err(config(
                &defaults_path,
                "/defaults/pageSize",
                "invalid page size",
            ));
        }
        let mut collections = BTreeMap::new();
        for directory in &directories {
            let schema_base = schema_bases.contains_key(&directory.path);
            if !schema_base
                && (!directory.direct_json
                    || schema_bases
                        .keys()
                        .any(|base| directory.path.starts_with(base)))
            {
                continue;
            }
            if schema_base
                && !directories
                    .iter()
                    .any(|entry| entry.direct_json && entry.path.starts_with(&directory.path))
            {
                return Err(config(
                    &directory.path,
                    "",
                    "collection must contain a JSON file",
                ));
            }
            let name = collection_name(&data, &directory.path)?;
            let schema_path = schema_bases.get(&directory.path).cloned();
            let (schema, schema_hash, fields, indexes, advanced_index_paths, sort_indexes) =
                if let Some(path) = &schema_path {
                    let path = fs::canonicalize(path).map_err(|error| io(path, error))?;
                    let crate::json::ParsedValue(raw) = read(&path, &mut files)?;
                    let schema = Schema::compile(&raw, &path)?;
                    let advanced_index_paths = crate::schema::index_paths(&raw, &path)?;
                    let sort_indexes = crate::schema::sort_indexes(&raw, &schema.field, &path)?;
                    let indexes = schema
                        .field
                        .properties
                        .iter()
                        .filter(|(_, field)| field.index)
                        .map(|(name, field)| IndexField {
                            name: name.clone(),
                            kind: field.kind,
                            nullable: field.nullable,
                        })
                        .collect();
                    let fields = schema.field.properties.clone();
                    input_collections
                        .entry(path)
                        .or_default()
                        .insert(name.clone());
                    (
                        Some(schema),
                        Some(fingerprint("schema/v1", &raw)),
                        fields,
                        indexes,
                        advanced_index_paths,
                        sort_indexes,
                    )
                } else {
                    (None, None, BTreeMap::new(), Vec::new(), Vec::new(), Vec::new())
                };
            let ui_candidate = root.join(".editor/collections").join(&name).join("ui.json");
            let (ui_path, mut ui) = if schema.is_some() && ui_candidate.is_file() {
                let ui: UiFile = read(&ui_candidate, &mut files)?;
                input_collections
                    .entry(ui_candidate.clone())
                    .or_default()
                    .insert(name.clone());
                (Some(ui_candidate), ui)
            } else {
                (
                    None,
                    UiFile {
                        label: None,
                        display: "@id".into(),
                        page_size: None,
                        list: ListOptions::default(),
                    },
                )
            };
            let page_size = ui.page_size.unwrap_or(default_page);
            if page_size == 0 || page_size > i64::MAX as usize {
                return Err(config(&directory.path, "/pageSize", "invalid page size"));
            }
            let ui_location = ui_path.as_deref().unwrap_or(&directory.path);
            scalar(&fields, &ui.display, ui_location)?;
            if ui.list.columns.is_empty() {
                ui.list.columns.push(ui.display.clone());
            }
            let order = complete_order(
                std::iter::once(ui.list.default_sort.clone())
                    .chain(ui.list.sub_sorts.clone())
                    .collect(),
            )?;
            if ui.list.sortable.is_none() {
                ui.list.sortable = Some(
                    order
                        .iter()
                        .filter(|sort| sort.field != "@id")
                        .map(|sort| sort.field.clone())
                        .collect(),
                );
            }
            let sortable = ui.list.sortable.as_ref().expect("view sortable is set");
            for names in [&ui.list.columns, &ui.list.filters, sortable] {
                unique(names, ui_location)?;
                for field in names {
                    scalar(&fields, field, ui_location)?;
                }
            }
            for sort in &order {
                scalar(&fields, &sort.field, ui_location)?;
            }
            let mapping_hash = fingerprint("mapping/v2", &(&directory.path, schema_base));
            let index_hash = fingerprint(
                "ordered-scalar-indexes/v3",
                &(&indexes, &advanced_index_paths, &sort_indexes),
            );
            let label = ui.label.unwrap_or_else(|| name.clone());
            let view_hashes = ViewHashes::new(
                &ui.list.filters,
                &(&order, &ui.list.sortable),
                &(&label, &ui.display, &ui.list.columns, page_size),
                &(&name, &schema_hash, &mapping_hash),
            );
            collections.insert(
                name.clone(),
                Collection {
                    name,
                    path: directory.path.clone(),
                    recursive: schema_base,
                    schema_path,
                    ui_path,
                    label,
                    display: ui.display,
                    page_size,
                    list: ui.list,
                    fields,
                    indexes,
                    advanced_index_paths,
                    sort_indexes,
                    schema_hash,
                    index_hash,
                    mapping_hash,
                    view_hashes,
                    schema,
                },
            );
        }
        if !collections.is_empty() {
            for path in files.keys() {
                if path == &defaults_path {
                    input_collections.insert(path.clone(), collections.keys().cloned().collect());
                }
            }
        }
        let graph = CollectionGraph::build(&collections)?;
        Ok(Self {
            root,
            collections,
            views: BTreeMap::new(),
            graph,
            files,
            input_collections,
        })
    }

    /// Load named collection schemas from the repository's `.objekte` directory.
    /// Populated JSON folders without a declaration remain discoverable.
    fn discover_objekte(root: &Path, settings: &Discovery) -> Result<Self> {
        let data = match &settings.data_root {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => root.join(path),
            None if root.join("data").is_dir() => root.join("data"),
            None => root.to_owned(),
        };
        let data = fs::canonicalize(&data).map_err(|error| io(&data, error))?;
        if !data.starts_with(root) || !data.is_dir() {
            return Err(config(&data, "", "data root must be inside repository"));
        }
        let mut directories = Vec::new();
        discover_directories(&data, &mut directories)?;
        let mut files = BTreeMap::new();
        let mut input_collections = BTreeMap::<PathBuf, BTreeSet<String>>::new();
        let mut collections = BTreeMap::new();
        let mut bases = BTreeMap::<PathBuf, String>::new();
        let schema_dir = root.join(".objekte/collections");
        let entries = if schema_dir.is_dir() {
            fs::read_dir(&schema_dir)
                .map_err(|error| io(&schema_dir, error))?
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|error| io(&schema_dir, error))?
        } else {
            Vec::new()
        };
        for entry in entries {
            let schema_path = entry.path();
            if !entry
                .file_type()
                .map_err(|error| io(&schema_path, error))?
                .is_file()
            {
                return Err(config(
                    &schema_path,
                    "",
                    "collection schemas must be regular files",
                ));
            }
            let Some(file_name) = schema_path.file_name().and_then(|name| name.to_str()) else {
                return Err(config(&schema_path, "", "schema filename must be UTF-8"));
            };
            let Some(file_name) = file_name.strip_suffix(".schema.json") else {
                return Err(config(
                    &schema_path,
                    "",
                    "expected <collection>.schema.json",
                ));
            };
            let file_name = file_name.to_owned();
            let crate::json::ParsedValue(raw) = read(&schema_path, &mut files)?;
            let metadata: CollectionMetadata =
                serde_json::from_value(raw.get("x-objekte").cloned().ok_or_else(|| {
                    config(
                        &schema_path,
                        "/x-objekte",
                        "collection metadata is required",
                    )
                })?)
                .map_err(|error| config(&schema_path, "/x-objekte", error))?;
            if metadata.name != file_name || metadata.name.is_empty() || metadata.name == "@root" {
                return Err(config(
                    &schema_path,
                    "/x-objekte/name",
                    "name must match the schema filename and cannot be @root",
                ));
            }
            if metadata.r#type != "data" || metadata.format != "json" || metadata.id != "filename" {
                return Err(config(
                    &schema_path,
                    "/x-objekte",
                    "only data/json/filename collections are supported",
                ));
            }
            let source = inside(root, &metadata.path)?;
            if !source.is_dir() || source == root || source.starts_with(root.join(".objekte")) {
                return Err(config(
                    &schema_path,
                    "/x-objekte/path",
                    "invalid source directory",
                ));
            }
            if !has_json(&source, true)? {
                return Err(config(
                    &schema_path,
                    "/x-objekte/path",
                    "collection must contain a JSON file",
                ));
            }
            if bases
                .keys()
                .any(|base| source.starts_with(base) || base.starts_with(&source))
            {
                return Err(config(
                    &schema_path,
                    "/x-objekte/path",
                    "schema base folders overlap",
                ));
            }
            let schema = Schema::compile(&raw, &schema_path)?;
            let advanced_index_paths = crate::schema::index_paths(&raw, &schema_path)?;
            let sort_indexes = crate::schema::sort_indexes(&raw, &schema.field, &schema_path)?;
            let indexes = schema
                .field
                .properties
                .iter()
                .filter(|(_, field)| field.index)
                .map(|(name, field)| IndexField {
                    name: name.clone(),
                    kind: field.kind,
                    nullable: field.nullable,
                })
                .collect::<Vec<_>>();
            let fields = schema.field.properties.clone();
            let schema_hash = Some(fingerprint("schema/v1", &raw));
            let index_hash = fingerprint(
                "ordered-scalar-indexes/v3",
                &(&indexes, &advanced_index_paths, &sort_indexes),
            );
            let mapping_hash = fingerprint(
                "mapping/v3",
                &(&source, &metadata.r#type, &metadata.format, &metadata.id),
            );
            input_collections
                .entry(schema_path.clone())
                .or_default()
                .insert(metadata.name.clone());
            bases.insert(source.clone(), metadata.name.clone());
            collections.insert(
                metadata.name.clone(),
                Collection {
                    name: metadata.name,
                    path: source,
                    recursive: true,
                    schema_path: Some(schema_path),
                    ui_path: None,
                    label: file_name,
                    display: "@id".into(),
                    page_size: 50,
                    list: ListOptions::default(),
                    fields,
                    indexes,
                    advanced_index_paths,
                    sort_indexes,
                    schema_hash,
                    index_hash,
                    mapping_hash: mapping_hash.clone(),
                    view_hashes: ViewHashes::new(&(), &(), &(), &mapping_hash),
                    schema: Some(schema),
                },
            );
        }
        for directory in &directories {
            if !directory.direct_json || bases.keys().any(|base| directory.path.starts_with(base)) {
                continue;
            }
            let name = collection_name(&data, &directory.path)?;
            if collections.contains_key(&name) {
                return Err(config(
                    &directory.path,
                    "",
                    "discovered collection name conflicts with a schema",
                ));
            }
            let mapping_hash =
                fingerprint("mapping/v3", &(&directory.path, "data", "json", "filename"));
            collections.insert(
                name.clone(),
                Collection {
                    name: name.clone(),
                    path: directory.path.clone(),
                    recursive: false,
                    schema_path: None,
                    ui_path: None,
                    label: name,
                    display: "@id".into(),
                    page_size: 50,
                    list: ListOptions::default(),
                    fields: BTreeMap::new(),
                    indexes: Vec::new(),
                    advanced_index_paths: Vec::new(),
                    sort_indexes: Vec::new(),
                    schema_hash: None,
                    index_hash: fingerprint("ordered-scalar-indexes/v3", &()),
                    mapping_hash: mapping_hash.clone(),
                    view_hashes: ViewHashes::new(&(), &(), &(), &mapping_hash),
                    schema: None,
                },
            );
        }
        let views = load_views(root, &mut files)?;
        let graph = CollectionGraph::build(&collections)?;
        Ok(Self {
            root: root.to_owned(),
            collections,
            views,
            graph,
            files,
            input_collections,
        })
    }
}


fn load_views(
    root: &Path,
    files: &mut BTreeMap<PathBuf, String>,
) -> Result<BTreeMap<String, crate::ViewDefinition>> {
    let directory = root.join(".objekte/views");
    if !directory.is_dir() {
        return Ok(BTreeMap::new());
    }
    let mut views = BTreeMap::new();
    for entry in fs::read_dir(&directory).map_err(|error| io(&directory, error))? {
        let entry = entry.map_err(|error| io(&directory, error))?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|error| io(&path, error))?
            .is_file()
        {
            return Err(config(&path, "", "view definitions must be regular files"));
        }
        let Some(name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".view.json"))
        else {
            return Err(config(&path, "", "expected <view>.view.json"));
        };
        let mut view: crate::ViewDefinition = read(&path, files)?;
        if view.name != name {
            return Err(config(&path, "/name", "view name must match its filename"));
        }
        view.path = path.clone();
        view.validate().map_err(|error| config(&path, "", error))?;
        views.insert(view.name.clone(), view);
    }
    Ok(views)
}

fn collection_name(data: &Path, directory: &Path) -> Result<String> {
    let relative = directory
        .strip_prefix(data)
        .expect("discovered data directory");
    if relative.as_os_str().is_empty() {
        return Ok("@root".into());
    }
    if relative == Path::new("@root") {
        return Err(config(
            directory,
            "",
            "@root is reserved for the data-root collection",
        ));
    }
    relative
        .to_str()
        .map(|name| name.replace(std::path::MAIN_SEPARATOR, "/"))
        .ok_or_else(|| config(directory, "", "collection path must be UTF-8"))
}

fn discover_directories(data: &Path, output: &mut Vec<Directory>) -> Result<()> {
    let mut pending = vec![data.to_owned()];
    while let Some(directory) = pending.pop() {
        let mut direct_json = false;
        for entry in fs::read_dir(&directory).map_err(|error| io(&directory, error))? {
            let entry = entry.map_err(|error| io(&directory, error))?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|error| io(&path, error))?;
            if kind.is_dir() {
                let name = entry.file_name();
                if matches!(name.to_str(), Some(".editor" | ".objekte" | ".git")) {
                    continue;
                }
                pending.push(path);
            } else if kind.is_file()
                && path.extension().and_then(|ext| ext.to_str()) == Some("json")
            {
                direct_json = true;
            }
        }
        output.push(Directory {
            path: directory,
            direct_json,
        });
    }
    output.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(())
}

pub(super) fn has_json(directory: &Path, recursive: bool) -> Result<bool> {
    let mut pending = vec![directory.to_owned()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).map_err(|error| io(&current, error))? {
            let entry = entry.map_err(|error| io(&current, error))?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|error| io(&path, error))?;
            if kind.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                return Ok(true);
            }
            if recursive
                && kind.is_dir()
                && !matches!(
                    entry.file_name().to_str(),
                    Some(".editor" | ".objekte" | ".git")
                )
            {
                pending.push(path);
            }
        }
    }
    Ok(false)
}
