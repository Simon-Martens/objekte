#!/usr/bin/env python3
"""Move a legacy Musenalm project's item and content records into entries."""
import argparse
import copy
import json
import pathlib
import shutil
import tempfile


def read(path):
    return json.loads(path.read_text(encoding='utf-8'))


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


def migrate(root):
    data = root / 'data'
    repository_path = root / '.editor/repository.json'
    repository = read(repository_path)
    if not all(name in repository['collections'] for name in ('items', 'contents')):
        raise ValueError('expected an unmigrated Musenalm project')

    entries = sorted((data / 'entries').glob('*.json'))
    used = {'items': set(), 'contents': set()}
    for path in entries:
        entry = read(path)
        for child in used:
            for identifier in entry[child]:
                if not isinstance(identifier, str) or identifier in used[child]:
                    raise ValueError(f'duplicate or invalid {child} reference: {identifier}')
                if not (data / child / f'{identifier}.json').is_file():
                    raise ValueError(f'missing {child}/{identifier}')
                used[child].add(identifier)
    for child in used:
        source_ids = {path.stem for path in (data / child).glob('*.json')}
        if source_ids != used[child]:
            raise ValueError(f'{child}: {len(source_ids - used[child])} unplaced records')

    schema_path = root / '.editor/collections/entries/schema.json'
    ui_path = root / '.editor/collections/entries/ui.json'
    report_path = root / 'import-report.json'
    schema, ui, report = map(read, (schema_path, ui_path, report_path))
    for child in used:
        child_root = root / '.editor/collections' / child
        embedded = copy.deepcopy(read(child_root / 'schema.json'))
        embedded.pop('$schema', None)
        embedded['properties'].pop('entries', None)
        embedded['properties'].pop('numbering', None)
        embedded['properties']['source_id'] = {'type': 'string', 'readOnly': True}
        embedded['required'] = [key for key in embedded['required'] if key not in ('entries', 'numbering')]
        schema['properties'][child] = {'type': 'array', 'default': [], 'items': embedded}
        prefix = f'/properties/{child}/items'
        for path, config in read(child_root / 'ui.json')['fields'].items():
            if path == '/properties/numbering':
                continue
            ui['fields'][prefix + path] = config
        ui['fields'][prefix + '/properties/source_id'] = {'title': 'Source id'}
        repository['collections'].pop(child)
        report.setdefault('embedded_records', {})[child] = report['records'].pop(child)

    with tempfile.TemporaryDirectory(prefix='.musenalm-embedded-', dir=data) as staging:
        staged = pathlib.Path(staging)
        for path in entries:
            entry = read(path)
            for child in used:
                children = []
                for identifier in entry[child]:
                    record = read(data / child / f'{identifier}.json')
                    numbering = record.pop('numbering', 0)
                    record['source_id'] = identifier
                    children.append((numbering, identifier, record))
                children.sort(key=lambda part: (part[0], part[1]))
                entry[child] = [part[2] for part in children]
            write(staged / path.name, entry)
        backup = data / '.entries-before-embed'
        if backup.exists():
            raise ValueError(f'{backup} already exists')
        (data / 'entries').rename(backup)
        staged.rename(data / 'entries')
    write(schema_path, schema)
    write(ui_path, ui)
    write(repository_path, repository)
    write(report_path, report)
    for child in used:
        shutil.rmtree(data / child)
        shutil.rmtree(root / '.editor/collections' / child)
    shutil.rmtree(backup)
    from musenalm_ui import apply
    apply(root)
    return {child: len(ids) for child, ids in used.items()}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('project', type=pathlib.Path, nargs='?', default=pathlib.Path('examples/musenalm'))
    print(migrate(parser.parse_args().project))
