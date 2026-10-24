import importlib.util
import json
import pathlib
import tempfile
import unittest
import xml.etree.ElementTree as ET

spec = importlib.util.spec_from_file_location('migration', pathlib.Path(__file__).with_name('import-musenalm.py'))
migration = importlib.util.module_from_spec(spec)
spec.loader.exec_module(migration)

class ImportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.source = pathlib.Path(self.temp.name) / 'source'
        self.source.mkdir()
        self.target = pathlib.Path(self.temp.name) / 'project'
        self.ids = {n: str(i).zfill(15) for i, n in enumerate(migration.NAMES + migration.RELATIONS, 1)}
        def field(name, kind='text', **extra):
            return {'name': name, 'type': kind, **extra}
        fields = {n: [field('id'), field(migration.LABEL[n], required=True)] for n in migration.NAMES}
        fields['entries'] += [field('series', 'relation', collectionId='series'), field('year', 'number'), field('language', 'select', maxSelect=5, values=['ger']), field('pseudonym', 'bool'), field('edit_fielddata', 'json')]
        fields['items'] += [field('entries', 'relation', collectionId='entries')]
        fields['contents'] += [field('entries', 'relation', collectionId='entries'), field('annotation', 'editor'), field('numbering', 'number')]
        for junction, parent, target in [('R_entries_series','series','entries'), ('R_entries_agents','entries','agents'), ('R_contents_agents','contents','agents')]:
            fields[junction] = [field('id'), field(parent,'relation',collectionId=parent), field(target,'relation',collectionId=target), field('annotation','editor'), field('uncertain','bool')]
        self.write('_collections', [{'id': n, 'name': n, 'fields': json.dumps(f)} for n, f in fields.items()])
        self.records = {n: {'id': self.ids[n], migration.LABEL[n]: 'Title & <em>source</em>'} for n in migration.NAMES}
        self.records['entries'].update(series=self.ids['series'],year='1805', language='["ger"]',pseudonym='false',edit_fielddata=None)
        self.records['items']['entries'] = self.ids['entries']
        self.records['contents'].update(entries=self.ids['entries'],annotation='First<br>second\n**third**',numbering='1.5')
        for n, r in self.records.items(): self.write(n, [r])
        for junction, parent, target in [('R_entries_series','series','entries'), ('R_entries_agents','entries','agents'), ('R_contents_agents','contents','agents')]:
            self.write(junction, [{'id': self.ids[junction], parent: self.ids[parent], target: self.ids[target], 'annotation': '<p>Relationship note</p>', 'uncertain':'true'}])
    def write(self, name, rows):
        root = ET.Element('table', name=name)
        for data in rows:
            row = ET.SubElement(root, 'row')
            for name, value in data.items():
                node = ET.SubElement(row, name)
                if value is None: node.set('null','true')
                else: node.text = value
        ET.ElementTree(root).write(self.source / (root.attrib['name'] + '.xml'), encoding='utf-8')
    def read(self, name):
        return json.loads((self.target/'data'/name/(self.ids[name]+'.json')).read_text())
    def test_typed_fields_and_distinct_relationship_semantics_survive(self):
        report = migration.import_project(self.source, self.target)
        series, entry, content = [self.read(n) for n in ('series','entries','contents')]
        self.assertEqual(series['entries'], [self.ids['entries']])
        self.assertEqual(series['related_entries'][0]['entry'], self.ids['entries'])
        self.assertEqual(series['related_entries'][0]['annotation'], '<p>Relationship note</p>')
        self.assertNotIn('series',entry)
        self.assertNotIn('entries',content)
        self.assertEqual(entry['contents'], [self.ids['contents']])
        self.assertEqual(entry['items'], [self.ids['items']])
        self.assertEqual(entry['language'], ['ger'])
        self.assertIs(entry['pseudonym'],False)
        self.assertIsNone(entry['edit_fielddata'])
        self.assertEqual(content['numbering'],1.5)
        self.assertEqual(content['annotation'],'First<br>second\n**third**')
        self.assertEqual(content['agents'][0]['agent'],self.ids['agents'])
        self.assertIs(content['agents'][0]['uncertain'],True)
        self.assertEqual(set(p.name for p in (self.target/'data').iterdir()),set(migration.NAMES))
        self.assertEqual(sum(report['records'].values()),6)
        with self.assertRaisesRegex(ValueError,'already exists'): migration.import_project(self.source,self.target)
    def test_missing_reference_fails_before_writing(self):
        self.records['items']['entries']='missing00000000'
        self.write('items',[self.records['items']])
        with self.assertRaisesRegex(ValueError,'missing entries'): migration.import_project(self.source,self.target)
        self.assertFalse(self.target.exists())
    def test_duplicate_ids_fail_before_writing(self):
        self.write('items',[self.records['items'],self.records['items']])
        with self.assertRaisesRegex(ValueError,'duplicate ID'): migration.import_project(self.source,self.target)
        self.assertFalse(self.target.exists())

if __name__ == '__main__': unittest.main()
