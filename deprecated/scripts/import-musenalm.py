#!/usr/bin/env python3
"""Import the supplied PocketBase XML export into a fresh JSON project.

Preserves source IDs (as filenames or embedded source_id fields), text including
embedded HTML, typed JSON, relationship attributes and source timestamps.
Never overwrites a project.
"""
import argparse
import collections
import copy
import hashlib
import json
import pathlib
import re
import shutil
import tempfile
import xml.etree.ElementTree as ET

TABLE_COLUMNS = {
    'contents': ['preferred_title', 'agents', 'musenalm_type', 'extent'],
    'items': ['identifier', 'owner', 'location', 'media'],
    'agents': ['agent', 'type', 'annotation'],
    'related_entries': ['entry', 'annotation'],
}


def row_defaults(schema):
    """Keep embedded row defaults from the original XML import."""
    for key, prop in schema.get('properties', {}).items():
        if prop.get('type') == 'array' and prop.get('items', {}).get('type') == 'object':
            item = prop['items']
            if key in TABLE_COLUMNS:
                initial = {}
                for column in TABLE_COLUMNS[key]:
                    kind = item['properties'].get(column, {}).get('type')
                    choices = item['properties'].get(column, {}).get('enum')
                    if kind in ('string', 'array') and (choices is None or '' in choices):
                        initial[column] = [] if kind == 'array' else ''
                item['default'] = initial
            row_defaults(item)
        elif prop.get('type') == 'object':
            row_defaults(prop)
    return schema

NAMES = ('series', 'entries', 'items', 'contents', 'agents', 'places')
PROJECT_NAMES = ('series', 'entries', 'agents', 'places')
RELATIONS = ('R_entries_series', 'R_entries_agents', 'R_contents_agents')
LABEL = dict(series='title', entries='preferred_title', items='identifier',
             contents='preferred_title', agents='name', places='name')
def rows(path):
    context = ET.iterparse(path, events=('start', 'end'))
    _, root = next(context)
    if root.tag != 'table':
        raise ValueError(f'{path}: expected table')
    for event, row in context:
        if event == 'end' and row.tag == 'row':
            result = {v.tag: None if v.get('null') == 'true' else (v.text or '') for v in row}
            if len(result) != len(row):
                raise ValueError(f'{path}: duplicate fields')
            yield result
            root.clear()


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


def convert(value, field):
    kind = field['type']
    if value is None:
        if kind != 'json':
            raise ValueError(f"Unexpected null: {field['name']}")
        return None
    if kind == 'bool':
        if value not in ('true', 'false'):
            raise ValueError(f'Invalid boolean {value}')
        return value == 'true'
    if kind == 'number':
        number = json.loads(value)
        if type(number) not in (int, float):
            raise ValueError(f'Invalid number {value}')
        return number
    if kind == 'json' or (kind in ('select', 'relation', 'file') and field.get('maxSelect', 1) > 1):
        return json.loads(value)
    return value


def field_schema(f, targets):
    kind = f['type']
    s = {'type': 'string'}
    if kind == 'bool':
        s = {'type': 'boolean'}
    elif kind == 'number':
        s = {'type': 'integer' if f.get('onlyInt') else 'number'}
        for old, new in [('min', 'minimum'), ('max', 'maximum')]:
            if f.get(old) is not None:
                s[new] = f[old]
    elif kind == 'json':
        s = {}  # Source JSON can contain arbitrary objects, arrays, scalars or null.
    elif kind == 'relation':
        target = targets[f['collectionId']]
        if target in NAMES:
            s['x-relation'] = {'collection': target}
        else:
            s['description'] = f'Source {target} ID; that collection was not included in the export.'
    elif kind == 'select':
        s['enum'] = f['values'] + ([] if f.get('required') or '' in f['values'] else [''])
    elif kind in ('editor', 'text'):
        # Markdown also preserves the export's inline HTML and line breaks.
        if kind == 'editor':
            s['x-content'] = 'markdown'
        for old, new in [('min', 'minLength'), ('max', 'maxLength')]:
            if f.get(old):
                s[new] = f[old]
        if f.get('pattern'):
            s['pattern'] = f['pattern']
        if f.get('required'):
            s['minLength'] = max(1, s.get('minLength', 0))
    elif kind not in ('file', 'url', 'autodate', 'date'):
        raise ValueError(f'Unsupported type {kind}')
    if kind in ('select', 'relation', 'file') and f.get('maxSelect', 1) > 1:
        s = {'type': 'array', 'items': s, 'maxItems': f['maxSelect'], 'uniqueItems': True}
    if kind == 'file':
        s['description'] = 'Original scan filenames. Binary files were not included in the export.'
    if f.get('help'):
        s['description'] = f['help']
    return s


def child_schema(target):
    return {'type': 'array', 'uniqueItems': True, 'default': [], 'x-children': True,
            'items': {'type': 'string', 'x-relation': {'collection': target}}}


def import_project(source, destination):
    if destination.exists():
        raise ValueError(f'{destination} already exists; choose a fresh destination')
    metadata = list(rows(source / '_collections.xml'))
    targets = {m['id']: m['name'] for m in metadata}
    fields = {m['name']: json.loads(m['fields']) for m in metadata if m['name'] in NAMES + RELATIONS}
    data = {}
    for name in NAMES + RELATIONS:
        expected = {f['name']: f for f in fields[name]}
        data[name] = {}
        for row in rows(source / (name + '.xml')):
            if set(row) != set(expected):
                raise ValueError(f'{name}: fields differ from metadata: {set(row) ^ set(expected)}')
            record = {k: convert(v, expected[k]) for k, v in row.items()}
            identifier = record.pop('id')
            if not re.fullmatch('[a-z0-9]{15}', identifier) or identifier in data[name]:
                raise ValueError(f'{name}: unsafe or duplicate ID {identifier}')
            data[name][identifier] = record
        print(f'{name}: {len(data[name]):,}', flush=True)
    # Check every relationship before transforming or writing any output.
    external = collections.Counter()
    for name, records in data.items():
        for f in fields[name]:
            if f['type'] != 'relation':
                continue
            target = targets[f['collectionId']]
            for identifier, record in records.items():
                value = record[f['name']]
                for ref in value if isinstance(value, list) else [value]:
                    if not ref:
                        continue
                    if target not in data:
                        external[target] += 1
                    elif ref not in data[target]:
                        raise ValueError(f'{name}/{identifier}: missing {target}/{ref}')
    schemas = {}
    for name in NAMES:
        props = {f['name']: field_schema(f, targets) for f in fields[name] if f['name'] != 'id'}
        for key in ('created', 'updated', 'editor'):
            if key in props:
                props[key]['readOnly'] = True
        required = [f['name'] for f in fields[name] if f.get('required') and f['name'] != 'id']
        schemas[name] = {'$schema': 'https://json-schema.org/draft/2020-12/schema',
                         'type': 'object', 'properties': props, 'required': required, 'additionalProperties': False}
    # Primary series placement lives on the parent; items and contents are
    # embedded below after their agent relationships have been assembled.
    placement_counts = {}
    for parent, child, old_field in [('series', 'entries', 'series')]:
        for record in data[parent].values():
            record[child] = []
        for identifier, record in data[child].items():
            owner = record.pop(old_field)
            if owner:
                data[parent][owner][child].append(identifier)
        schemas[parent]['properties'][child] = child_schema(child)
        schemas[child]['properties'].pop(old_field)
        schemas[child]['required'] = [k for k in schemas[child]['required'] if k != old_field]
        for record in data[parent].values():
            record[child].sort(key=lambda k: (data[child][k].get('numbering', data[child][k].get('year', 0)), k))
        placement_counts[f'{parent}.{child}'] = sum(len(r[child]) for r in data[parent].values())
    for junction, parent, target, field, ref in [
        ('R_entries_series', 'series', 'entries', 'related_entries', 'entry'),
        ('R_entries_agents', 'entries', 'agents', 'agents', 'agent'),
        ('R_contents_agents', 'contents', 'agents', 'agents', 'agent'),
    ]:
        relation_props = {}
        for f in fields[junction]:
            if f['name'] == parent:
                continue
            key = 'source_id' if f['name'] == 'id' else ref if f['name'] == target else f['name']
            relation_props[key] = field_schema(f, targets)
            if key in ('source_id', 'created', 'updated'):
                relation_props[key]['readOnly'] = True
        schemas[parent]['properties'][field] = {'type': 'array', 'default': [], 'items': {
            'type': 'object', 'properties': relation_props, 'required': [ref], 'additionalProperties': False}}
        for record in data[parent].values():
            record[field] = []
        for identifier, original in data[junction].items():
            record = dict(original)
            owner = record.pop(parent)
            record[ref] = record.pop(target)
            record['source_id'] = identifier
            data[parent][owner][field].append(record)
        for record in data[parent].values():
            record[field].sort(key=lambda r: r['source_id'])
    for child in ('items', 'contents'):
        embedded = copy.deepcopy(schemas[child])
        embedded.pop('$schema')
        embedded['properties'].pop('numbering', None)
        embedded['properties']['source_id'] = {'type': 'string', 'readOnly': True}
        embedded['properties'].pop('entries')
        embedded['required'] = [key for key in embedded['required'] if key not in ('entries', 'numbering')]
        schemas['entries']['properties'][child] = {'type': 'array', 'default': [], 'items': embedded}
        for record in data['entries'].values():
            record[child] = []
        for identifier, original in data[child].items():
            record = dict(original)
            owner = record.pop('entries')
            if not owner:
                raise ValueError(f'{child}/{identifier}: missing entry')
            numbering = record.pop('numbering', 0)
            record['source_id'] = identifier
            data['entries'][owner][child].append((numbering, identifier, record))
        for record in data['entries'].values():
            record[child].sort(key=lambda item: (item[0], item[1]))
            record[child] = [item[2] for item in record[child]]
        placement_counts[f'entries.{child}'] = len(data[child])
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = pathlib.Path(tempfile.mkdtemp(prefix='.musenalm-import-', dir=destination.parent))
    try:
        for name in PROJECT_NAMES:
            position = NAMES.index(name)
            schema = row_defaults(schemas[name])
            schema['x-objekte'] = {
                'name': name, 'path': 'data/' + name,
                'type': 'data', 'format': 'json', 'id': 'filename',
            }
            write(temporary / '.objekte/collections' / f'{name}.schema.json', schema)
            (temporary / 'data' / name).mkdir(parents=True)
            for identifier, record in data[name].items():
                write(temporary / 'data' / name / (identifier + '.json'), record)
        report = {'records': {n: len(data[n]) for n in PROJECT_NAMES},
                  'embedded_records': {n: len(data[n]) for n in ('items', 'contents')},
                  'embedded_relations': {n: len(data[n]) for n in RELATIONS},
                  'parent_placements': placement_counts, 'external_source_references': dict(external),
                  'source_sha256': {p.name: digest(p) for p in sorted(source.glob('*.xml'))}}
        write(temporary / 'import-report.json', report)
        temporary.rename(destination)
    except BaseException:
        shutil.rmtree(temporary)
        raise
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=pathlib.Path, nargs='?', default=pathlib.Path('import'))
    parser.add_argument('destination', type=pathlib.Path, nargs='?', default=pathlib.Path('examples/musenalm'))
    args = parser.parse_args()
    print(json.dumps(import_project(args.source, args.destination), ensure_ascii=False, indent=2))
