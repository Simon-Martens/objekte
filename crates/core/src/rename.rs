//! Record renames update stored references under the repository write lock.
use super::*;
use pulldown_cmark::LinkType;
use std::ops::Range;

#[derive(Debug, Clone, Serialize)]
pub struct RenameResult {
    pub document: Document,
    pub references_renamed: usize,
}

struct Source {
    original: Vec<u8>,
    value: Value,
    collection: Option<(String, String)>,
    mappings: Vec<String>,
    settings: bool,
    references: usize,
}

impl Editor {
    pub fn rename(
        &self,
        collection: &str,
        old_id: &str,
        new_id: &str,
        value: Value,
        expected: &str,
    ) -> Result<Document> {
        Ok(self
            .rename_with_references(collection, old_id, new_id, value, expected)?
            .document)
    }

    pub fn rename_with_references(
        &self,
        collection: &str,
        old_id: &str,
        new_id: &str,
        value: Value,
        expected: &str,
    ) -> Result<RenameResult> {
        if old_id == new_id {
            return Ok(RenameResult {
                document: self.save(collection, old_id, value, Some(expected))?,
                references_renamed: 0,
            });
        }
        let _guard = self.write_guard.lock().unwrap();
        self.require_operation(collection, "rename")?;
        self.require_operation(collection, "update")?;
        let id_issues = self.validate_new_id(collection, new_id)?;
        if !id_issues.is_empty() {
            return Err(Error::Validation(id_issues));
        }
        let old_path = self.find_path(collection, old_id)?;
        self.guard_document_location(&old_path)?;
        let previous = self.get(collection, old_id)?;
        if previous.revision != expected {
            return Err(Error::Conflict("record changed on disk".into()));
        }
        let mut value = self.prepare_save(collection, new_id, value, Some(&previous.value))?;
        let config = self.config.collection(collection)?;
        if let IdStrategy::Frontmatter(field) = &config.id {
            value
                .as_object_mut()
                .ok_or_else(|| Error::Data("record must be an object".into()))?
                .insert(field.clone(), Value::String(new_id.into()));
        }
        let new_path = self
            .config
            .collection_dir(collection)?
            .join(format!("{new_id}.{}", extension(config)));
        if new_path != old_path {
            check_bytes(&new_path, None)?;
        }

        let signature = self.reference_signature()?;
        let mut sources = BTreeMap::new();
        for (name, config) in &self.config.collections {
            for path in self.files(name)? {
                let original = fs::read(&path)?;
                if path == old_path && hash(&original) != expected {
                    return Err(Error::Conflict("record changed on disk".into()));
                }
                let stored = parse(config, &original)?;
                let id = record_id(config, &path, &stored)?;
                sources.insert(
                    path.clone(),
                    Source {
                        original,
                        value: if path == old_path {
                            value.clone()
                        } else {
                            stored
                        },
                        collection: Some((name.clone(), id)),
                        mappings: Vec::new(),
                        settings: false,
                        references: 0,
                    },
                );
            }
        }
        for (name, mapping) in &self.config.documents {
            for file in &mapping.files {
                json_source(&mut sources, &self.config.root, file)?
                    .mappings
                    .push(name.clone());
            }
        }
        if let Some(settings) = &self.config.settings {
            json_source(&mut sources, &self.config.root, &settings.path)?.settings = true;
        }
        for source in sources.values_mut() {
            if let Some((name, _)) = &source.collection {
                source.references += rewrite_relations(
                    &self.config.collection(name)?.schema_value,
                    &mut source.value,
                    collection,
                    old_id,
                    new_id,
                )?;
            }
            for name in &source.mappings {
                source.references += rewrite_relations(
                    self.document_schema(name)?,
                    &mut source.value,
                    collection,
                    old_id,
                    new_id,
                )?;
            }
            if source.settings {
                source.references += rewrite_relations(
                    self.settings_schema()?,
                    &mut source.value,
                    collection,
                    old_id,
                    new_id,
                )?;
            }
            source.references += rewrite_markdown(&mut source.value, collection, old_id, new_id)?;
        }
        sources.retain(|path, source| path == &old_path || source.references > 0);

        // Validate against the final identities without exposing partially renamed
        // records to readers or temporarily writing an unvalidated target to disk.
        let mut validation = self.clone();
        validation
            .validation_records
            .insert((collection.into(), old_id.into()), None);
        for (path, source) in &mut sources {
            if let Some((name, id)) = &source.collection {
                let target_id = if path == &old_path { new_id } else { id };
                if path != &old_path {
                    self.require_operation(name, "update")?;
                    let original = parse(self.config.collection(name)?, &source.original)?;
                    source.value =
                        self.prepare_save(name, id, source.value.clone(), Some(&original))?;
                } else {
                    self.check_record_ownership(name, &value, &source.value)?;
                }
                validation.validation_records.insert(
                    (name.clone(), target_id.into()),
                    Some(Document {
                        id: target_id.into(),
                        value: source.value.clone(),
                        revision: String::new(),
                    }),
                );
            }
        }
        for source in sources.values() {
            let mut issues = Vec::new();
            if let Some((name, _)) = &source.collection {
                issues.extend(validation.validate(name, &source.value)?);
            }
            for name in &source.mappings {
                issues.extend(validation.validate_document(name, &source.value)?);
            }
            if source.settings {
                issues.extend(validation.validate_settings(&source.value)?);
            }
            if !issues.is_empty() {
                return Err(Error::Validation(issues));
            }
        }

        let references_renamed = sources.values().map(|source| source.references).sum();
        let mut changes = Vec::new();
        let mut document = None;
        for (path, source) in &sources {
            let bytes = if let Some((name, _)) = &source.collection {
                serialize(self.config.collection(name)?, &source.value)?
            } else {
                let mut bytes = serde_json::to_vec_pretty(&source.value)
                    .map_err(|e| Error::Data(e.to_string()))?;
                bytes.push(b'\n');
                bytes
            };
            if path == &old_path {
                document = Some(Document {
                    id: new_id.into(),
                    value: source.value.clone(),
                    revision: hash(&bytes),
                });
                changes.insert(
                    0,
                    Change {
                        path: new_path.clone(),
                        before: (old_path == new_path).then(|| source.original.clone()),
                        after: Some(bytes),
                    },
                );
            } else {
                changes.push(Change {
                    path: path.clone(),
                    before: Some(source.original.clone()),
                    after: Some(bytes),
                });
            }
        }
        if old_path != new_path {
            changes.push(Change {
                path: old_path.clone(),
                before: Some(sources[&old_path].original.clone()),
                after: None,
            });
        }
        // Also catch external additions/removals of referencing records during planning.
        if self.reference_signature()? != signature {
            return Err(Error::Conflict(
                "repository changed on disk during rename; retry the save".into(),
            ));
        }
        commit(&changes)?;
        for source in sources.values() {
            if let Some((name, _)) = &source.collection {
                self.clear_field_values(name);
            }
        }
        Ok(RenameResult {
            document: document.unwrap(),
            references_renamed,
        })
    }
}

fn json_source<'a>(
    sources: &'a mut BTreeMap<PathBuf, Source>,
    root: &Path,
    file: &str,
) -> Result<&'a mut Source> {
    reject_symlink(&root.join(file))?;
    let path = crate::contained(root, file)?;
    if !sources.contains_key(&path) {
        let original = fs::read(&path)?;
        let value =
            serde_json::from_slice(&original).map_err(|e| Error::Data(format!("{file}: {e}")))?;
        sources.insert(
            path.clone(),
            Source {
                original,
                value,
                collection: None,
                mappings: Vec::new(),
                settings: false,
                references: 0,
            },
        );
    }
    Ok(sources.get_mut(&path).unwrap())
}

fn rewrite_relations(
    schema: &Value,
    value: &mut Value,
    collection: &str,
    old: &str,
    new: &str,
) -> Result<usize> {
    let mut paths = BTreeSet::new();
    visit_relations(schema, schema, value, "", 0, &mut |target, id, _, path| {
        if target == collection && id == old {
            paths.insert(path.to_owned());
        }
        Ok(())
    })?;
    for path in &paths {
        *value.pointer_mut(path).unwrap() = Value::String(new.into());
    }
    Ok(paths.len())
}

fn rewrite_markdown(value: &mut Value, collection: &str, old: &str, new: &str) -> Result<usize> {
    match value {
        Value::Object(values) => values.values_mut().try_fold(0, |count, value| {
            Ok(count + rewrite_markdown(value, collection, old, new)?)
        }),
        Value::Array(values) => values.iter_mut().try_fold(0, |count, value| {
            Ok(count + rewrite_markdown(value, collection, old, new)?)
        }),
        Value::String(text) => rewrite_links(text, collection, old, new),
        _ => Ok(0),
    }
}

fn rewrite_links(text: &mut String, collection: &str, old: &str, new: &str) -> Result<usize> {
    let mut parser = Parser::new(text).into_offset_iter();
    let events: Vec<_> = parser.by_ref().collect();
    let definitions = parser.reference_definitions();
    let mut replacements = BTreeMap::new();
    let mut count = 0;
    for (index, (event, span)) in events.iter().enumerate() {
        let (kind, destination, id) = match event {
            Event::Start(
                Tag::Link {
                    link_type,
                    dest_url,
                    id,
                    ..
                }
                | Tag::Image {
                    link_type,
                    dest_url,
                    id,
                    ..
                },
            ) => (link_type, dest_url, id),
            _ => continue,
        };
        if record_link_target(destination).as_ref() != Some(&(collection.into(), old.into())) {
            continue;
        }
        let destination_range = match kind {
            LinkType::Inline => {
                let mut depth = 1;
                let mut label_end = span.start;
                for (child, range) in &events[index + 1..] {
                    match child {
                        Event::Start(_) => depth += 1,
                        Event::End(_) => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        break;
                    }
                    label_end = label_end.max(range.end);
                }
                text[label_end..span.end]
                    .find("](")
                    .map(|offset| label_end + offset + 2)
                    .and_then(|start| link_destination(text, start, span.end))
            }
            LinkType::Reference | LinkType::Collapsed | LinkType::Shortcut => {
                definitions.get(id).and_then(|definition| {
                    let start = definition.span.start;
                    text[start..definition.span.end]
                        .match_indices("]:")
                        .find(|(offset, _)| !escaped(text, start + offset))
                        .and_then(|(offset, _)| {
                            link_destination(text, start + offset + 2, definition.span.end)
                        })
                })
            }
            _ => None,
        }
        .ok_or_else(|| Error::Data("could not locate a Markdown record link for rename".into()))?;
        let prefix_len = if destination.starts_with('#') { 1 } else { 0 };
        let suffix = destination[prefix_len..]
            .find(['?', '#'])
            .map(|at| at + prefix_len)
            .unwrap_or(destination.len());
        let last_slash = destination[..suffix].rfind('/').unwrap();
        let encoded =
            percent_encoding::utf8_percent_encode(new, percent_encoding::NON_ALPHANUMERIC);
        let replacement = format!(
            "{}{encoded}{}",
            &destination[..last_slash + 1],
            &destination[suffix..]
        );
        replacements.insert(
            (destination_range.start, destination_range.end),
            replacement,
        );
        count += 1;
    }
    for ((start, end), replacement) in replacements.into_iter().rev() {
        text.replace_range(start..end, &replacement);
    }
    Ok(count)
}

fn escaped(text: &str, index: usize) -> bool {
    text.as_bytes()[..index]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count()
        % 2
        == 1
}

fn link_destination(text: &str, start: usize, end: usize) -> Option<Range<usize>> {
    let bytes = text.as_bytes();
    let mut start = start;
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    if start == end {
        return None;
    }
    let angled = bytes[start] == b'<';
    if angled {
        start += 1;
    }
    let mut depth = 0;
    let mut cursor = start;
    while cursor < end {
        let byte = bytes[cursor];
        if byte == b'\\' {
            cursor += 2;
            continue;
        }
        if angled {
            if byte == b'>' {
                break;
            }
        } else if byte.is_ascii_whitespace() || byte == b')' && depth == 0 {
            break;
        } else if byte == b'(' {
            depth += 1;
        } else if byte == b')' {
            depth -= 1;
        }
        cursor += 1;
    }
    (cursor > start && cursor <= end).then_some(start..cursor)
}

struct Change {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}
struct Staged(Vec<PathBuf>);
impl Drop for Staged {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}
fn check_bytes(path: &Path, expected: Option<&[u8]>) -> Result<()> {
    reject_symlink(path)?;
    match (fs::read(path), expected) {
        (Ok(bytes), Some(expected)) if bytes == expected => Ok(()),
        (Err(error), None) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(Error::Conflict(format!(
            "{} changed on disk",
            path.display()
        ))),
    }
}

// Stage every replacement before touching originals. Ordinary I/O failures roll
// back completed changes; as with other multi-file operations, this is not a
// crash-atomic filesystem transaction. Keep the old record until references land.
fn commit(changes: &[Change]) -> Result<()> {
    let mut staged = Staged(Vec::new());
    let mut files = Vec::new();
    for change in changes {
        check_bytes(&change.path, change.before.as_deref())?;
        if let Some(bytes) = &change.after {
            let path = change
                .path
                .parent()
                .unwrap()
                .join(format!(".editor-{}.tmp", uuid::Uuid::new_v4()));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            staged.0.push(path.clone());
            if change.before.is_some() {
                file.set_permissions(fs::metadata(&change.path)?.permissions())?;
            }
            file.write_all(bytes)?;
            file.sync_all()?;
            files.push(Some(path));
        } else {
            files.push(None);
        }
    }
    let mut attempted = 0;
    let result = (|| -> Result<()> {
        for (change, staged) in changes.iter().zip(&files) {
            check_bytes(&change.path, change.before.as_deref())?;
            attempted += 1;
            match (staged, &change.before) {
                (Some(staged), None) => fs::hard_link(staged, &change.path)?,
                (Some(staged), Some(_)) => fs::rename(staged, &change.path)?,
                (None, _) => fs::remove_file(&change.path)?,
            }
            fs::File::open(change.path.parent().unwrap())?.sync_all()?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut failures = Vec::new();
        for change in changes[..attempted].iter().rev() {
            if check_bytes(&change.path, change.before.as_deref()).is_ok() {
                continue;
            }
            let rollback = (|| -> Result<()> {
                check_bytes(&change.path, change.after.as_deref())?;
                match (&change.before, &change.after) {
                    (Some(bytes), None) => atomic_create(&change.path, bytes),
                    (Some(bytes), Some(_)) => atomic_write(&change.path, bytes),
                    (None, _) => {
                        fs::remove_file(&change.path)?;
                        Ok(())
                    }
                }
            })();
            if let Err(failure) = rollback {
                failures.push(failure.to_string());
            }
        }
        if !failures.is_empty() {
            return Err(Error::Data(format!(
                "{error}; rename rollback incomplete: {}",
                failures.join("; ")
            )));
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_renames_only_link_destinations_and_preserves_source() {
        for (before, after, count) in [
            (
                "[old](/collections/people/records/old \"old\")",
                "[old](/collections/people/records/new%20id \"old\")",
                1,
            ),
            (
                "[](/collections/people/records/old)",
                "[](/collections/people/records/new%20id)",
                1,
            ),
            (
                "[**text** and `](x)`](/collections/people/records/old)",
                "[**text** and `](x)`](/collections/people/records/new%20id)",
                1,
            ),
            (
                "![image](<#/collections/people/old>)",
                "![image](<#/collections/people/new%20id>)",
                1,
            ),
            (
                "[old](/collections/people/records/old?node=%2Fname#section)",
                "[old](/collections/people/records/new%20id?node=%2Fname#section)",
                1,
            ),
            (
                "[one][ref] [ref][] [ref]\n\n[ref]: /collections/people/records/old 'title'\n",
                "[one][ref] [ref][] [ref]\n\n[ref]: /collections/people/records/new%20id 'title'\n",
                3,
            ),
            (
                "[ref]\n\n[ref]:\n  </collections/people/records/old>\n  \"title\"\n",
                "[ref]\n\n[ref]:\n  </collections/people/records/new%20id>\n  \"title\"\n",
                1,
            ),
            (
                "[link](/collections/people/records/o&#108;d)",
                "[link](/collections/people/records/new%20id)",
                1,
            ),
            (
                "`[code](/collections/people/records/old)`\n\n```md\n[code](/collections/people/records/old)\n```",
                "`[code](/collections/people/records/old)`\n\n```md\n[code](/collections/people/records/old)\n```",
                0,
            ),
            (
                "[other](/collections/notes/records/old) [external](https://example.com/collections/people/records/old)",
                "[other](/collections/notes/records/old) [external](https://example.com/collections/people/records/old)",
                0,
            ),
        ] {
            let mut actual = before.to_owned();
            assert_eq!(
                rewrite_links(&mut actual, "people", "old", "new id").unwrap(),
                count,
                "{before}"
            );
            assert_eq!(actual, after, "{before}");
        }
    }

    #[test]
    fn late_conflicts_roll_back_prior_writes_and_remove_new_target() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("old.json");
        let target = temp.path().join("new.json");
        let source = temp.path().join("source.json");
        fs::write(&old, b"old record").unwrap();
        fs::write(&source, b"original reference").unwrap();
        let changes = [
            Change {
                path: target.clone(),
                before: None,
                after: Some(b"new record".to_vec()),
            },
            Change {
                path: source.clone(),
                before: Some(b"original reference".to_vec()),
                after: Some(b"updated reference".to_vec()),
            },
            // A later precondition no longer matches after an earlier write.
            Change {
                path: source.clone(),
                before: Some(b"original reference".to_vec()),
                after: None,
            },
            Change {
                path: old.clone(),
                before: Some(b"old record".to_vec()),
                after: None,
            },
        ];
        assert!(matches!(commit(&changes), Err(Error::Conflict(_))));
        assert_eq!(fs::read(&old).unwrap(), b"old record");
        assert_eq!(fs::read(&source).unwrap(), b"original reference");
        assert!(!target.exists());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }
}
