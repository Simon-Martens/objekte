"""Presentation defaults for the imported Musenalm catalogue."""
import json
import pathlib

LOCALES = json.loads(pathlib.Path(__file__).with_name("musenalm_locales.json").read_text())
LANGUAGES = {"defaultLanguage": "en", "languages": {"en": "English", "de": "Deutsch"}}

TITLES = {
    'preferred_title': 'Title', 'title_statement': 'Title as printed',
    'responsibility_statement': 'Attribution as printed', 'place_statement': 'Place as printed',
    'publication_statement': 'Publication', 'incipit_statement': 'Opening words',
    'subtitle_statement': 'Subtitle', 'variant_title': 'Alternative title',
    'parallel_title': 'Parallel title', 'musenalm_type': 'Type', 'musenalm_pagination': 'Pagination',
    'musenalm_id': 'Musenalm ID', 'musenalm_deprecated': 'Original catalogue data',
    'edit_fielddata': 'Import details', 'edit_state': 'Review status', 'edit_comment': 'Editorial notes',
    'annotation': 'Notes', 'refs': 'References', 'uri': 'Web link', 'agent': 'Name',
    'type': 'Role', 'agents': 'Agents', 'items': 'Items', 'contents': 'Contents',
    'source_id': 'Source ID', 'identifier': 'Shelfmark', 'owner': 'Holding institution',
    'media': 'Format', 'extent': 'Extent / pages', 'related_entries': 'Cross-references',
}
SECONDARY = {'source_id', 'created', 'updated', 'editor', 'edit_fielddata', 'musenalm_deprecated',
             'musenalm_id', 'edit_state', 'edit_comment', 'scans', 'data'}


EDITOR_SETTINGS_SCHEMA = {'$schema': 'https://json-schema.org/draft/2020-12/schema',
 'type': 'object',
 'additionalProperties': True,
 'title': 'Editor instance settings',
 'properties': {'defaultTheme': {'type': 'string',
                                 'title': 'Default color theme',
                                 'enum': ['system', 'light', 'dark'],
                                 'default': 'system',
                                 'description': 'Used when a browser has no personal theme choice.',
                                 'x-optionLabels': {'system': 'Follow device',
                                                    'light': 'Light',
                                                    'dark': 'Dark'}},
                'density': {'type': 'string',
                            'title': 'Interface density',
                            'enum': ['compact', 'comfortable'],
                            'default': 'compact',
                            'description': 'Spacing in lists and editor controls.',
                            'x-optionLabels': {'compact': 'Compact', 'comfortable': 'Comfortable'}},
                'pageSize': {'type': 'integer',
                             'title': 'Records per page',
                             'minimum': 10,
                             'maximum': 200,
                             'default': 50,
                             'description': 'Rows in collection lists. Collection UI settings can '
                                            'override this default.'},
                'editorFontSize': {'type': 'integer',
                                   'title': 'Source editor font size',
                                   'minimum': 11,
                                   'maximum': 20,
                                   'default': 13,
                                   'description': 'Text size in JSON, Markdown and HTML source '
                                                  'editors, in pixels.'},
                'validationDelay': {'type': 'integer',
                                    'title': 'Live validation delay (ms)',
                                    'minimum': 150,
                                    'maximum': 2000,
                                    'default': 400,
                                    'description': 'Milliseconds after typing before validation '
                                                   'runs.'},
                'markdownPreview': {'type': 'boolean',
                                    'title': 'Open Markdown previews by default',
                                    'default': False,
                                    'description': 'Show the rendered preview when opening a '
                                                   'Markdown field.'},
                'mediaThumbnails': {'type': 'boolean',
                                    'title': 'Show image thumbnails in media lists',
                                    'default': True,
                                    'description': 'Show small image previews beside files in '
                                                   'media collections.'},
                'tablePageSize': {'type': 'integer',
                                  'title': 'Rows per table page',
                                  'minimum': 1,
                                  'maximum': 100,
                                  'default': 25,
                                  'description': 'Rows in an open Contents, Items or other '
                                                 'embedded table, including the edit form. A '
                                                 'field-specific page size takes precedence.'}}}


TABLE_ICONS = {'agents': 'ri-user-line', 'items': 'ri-archive-line',
               'contents': 'ri-file-list-3-line', 'related_entries': 'ri-book-line'}

TABLE_COLUMNS = {
    'contents': ['preferred_title', 'agents', 'musenalm_type', 'extent'],
    'items': ['identifier', 'owner', 'location', 'media'],
    'agents': ['agent', 'type', 'annotation'],
    'related_entries': ['entry', 'annotation'],
}


def row_defaults(schema):
    """New embedded rows start with their primary fields ready to edit."""
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


def configure(name, schema, ui):
    ui.setdefault('presentation', {})['hideEmpty'] = True
    ui['sidebarTree'] = False
    ui['list']['columns'] = {
        'entries': ['preferred_title', 'year', 'places', 'contents', 'items'],
        'series': ['title', 'frequency', 'entries'],
        'agents': ['name', 'profession', 'biographical_data'],
        'places': ['name', 'uri'],
    }[name]
    ui['list']['filters'] = ['edit_state']
    def visit(node, pointer=''):
        if pointer and 'properties' in node:
            order = ['preferred_title', 'identifier', 'agent', 'entry', 'type', 'owner', 'location', 'media', 'musenalm_type', 'extent', 'incipit_statement', 'year', 'language', 'title_statement', 'responsibility_statement', 'publication_statement', 'annotation']
            ordered = [key for key in order if key in node['properties']] + [key for key in node['properties'] if key not in order]
            ui.setdefault('fields', {}).setdefault(pointer, {}).setdefault('x-presentation', {})['fieldOrder'] = ordered
        for key, prop in node.get('properties', {}).items():
            path = pointer + '/properties/' + key
            conf = ui.setdefault('fields', {}).setdefault(path, {})
            conf['title'] = TITLES.get(key, key.replace('_', ' ').capitalize())
            array_name = pointer.rsplit('/properties/', 1)[-1].removesuffix('/items')
            conf.get('x-presentation', {}).pop('primary', None)
            is_table = prop.get('type') == 'array' and prop.get('items', {}).get('type') == 'object'
            if key != 'annotation' and key in TABLE_COLUMNS.get(array_name, []) and not is_table:
                conf.setdefault('x-presentation', {})['primary'] = True
            if key in ('places', 'language', 'musenalm_type', 'pseudonyms'):
                conf.setdefault('x-presentation', {})['inline'] = True
            if key in ('title_statement', 'responsibility_statement', 'incipit_statement', 'publication_statement', 'biographical_data', 'condition'):
                conf.setdefault('x-presentation', {})['mode'] = 'multiline'
            if key == 'uri':
                conf.setdefault('x-presentation', {})['link'] = True
            if key in SECONDARY:
                conf.setdefault('x-presentation', {})['secondary'] = True
            if key in ('preferred_title', 'name', 'title') and not pointer:
                conf.setdefault('x-presentation', {})['preview'] = 'hidden'
            if prop.get('type') == 'array' and prop.get('items', {}).get('type') == 'object':
                columns = TABLE_COLUMNS.get(key)
                if columns:
                    columns = [column for column in columns if column in prop['items']['properties']]
                    conf.setdefault('x-presentation', {})['table'] = {'columns': columns, 'icon': TABLE_ICONS.get(key, 'ri-table-line')}
                visit(prop['items'], path + '/items')
            elif prop.get('type') == 'object':
                visit(prop, path)
    visit(schema)
    priorities = {
        'entries': ['preferred_title', 'year', 'places', 'language', 'agents', 'items', 'contents', 'annotation', 'title_statement', 'responsibility_statement', 'publication_statement', 'refs'],
        'series': ['title', 'frequency', 'annotation', 'refs', 'related_entries'],
        'agents': ['name', 'biographical_data', 'profession', 'pseudonyms', 'uri', 'annotation', 'refs'],
        'places': ['name', 'pseudonyms', 'uri', 'annotation'],
    }[name]
    ui['fieldOrder'] = [key for key in priorities if key in schema['properties']] + [key for key in schema['properties'] if key not in priorities]
    ui['translations'] = LOCALES[name]['translations']
    for pointer, text in LOCALES[name]['fields'].items():
        node = schema
        try:
            for part in pointer.strip('/').split('/'):
                node = node[part.replace('~1', '/').replace('~0', '~')]
        except (KeyError, TypeError):
            continue
        ui.setdefault('fields', {}).setdefault(pointer, {}).update(text)
    return ui


def apply(root):
    global_path = root / '.editor/ui.json'
    global_ui = json.loads(global_path.read_text())
    global_ui['i18n'] = LANGUAGES
    for group in global_ui.get('groups', []):
        if group.get('name') == 'Catalogue': group['translations'] = {'de': {'label': 'Katalog'}}
    global_path.write_text(json.dumps(global_ui, ensure_ascii=False, indent=2) + '\n')
    settings_path = root / '.editor/settings.schema.json'
    settings = json.loads(settings_path.read_text()) if settings_path.exists() else {}
    settings.update({key: value for key, value in EDITOR_SETTINGS_SCHEMA.items() if key != 'properties'})
    settings.setdefault('properties', {}).update(EDITOR_SETTINGS_SCHEMA['properties'])
    settings_path.write_text(json.dumps(settings, ensure_ascii=False, indent=2) + '\n')
    for name in ('series', 'entries', 'agents', 'places'):
        directory = root / '.editor/collections' / name
        schema = json.loads((directory / 'schema.json').read_text())
        path = directory / 'ui.json'
        ui = json.loads(path.read_text())
        for field in ui.get('fields', {}).values():
            field.pop('default', None)
        ui = configure(name, schema, ui)
        (directory / 'schema.json').write_text(json.dumps(row_defaults(schema), ensure_ascii=False, indent=2) + '\n')
        path.write_text(json.dumps(ui, ensure_ascii=False, indent=2) + '\n')


if __name__ == '__main__':
    apply(pathlib.Path('examples/musenalm'))
