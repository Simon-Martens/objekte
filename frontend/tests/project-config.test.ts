import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// @ts-ignore Import scripts are plain JavaScript.
import { splitConfig, readProjectDefinition, writeProjectDefinition } from '../scripts/project-config.mjs';

const definition = {
  'x-project': { media: 'media' }, anyOf: [{
    'x-collection': { name: 'notes', path: 'data/notes', format: 'json', id: 'filename', label: 'Notes', display: 'text', search: ['text'] },
    type: 'object', properties: { text: { type: 'string', title: 'Text', 'x-editor': 'inline-markdown' } },
  }],
};
test('split keeps storage, field semantics and presentation separate', () => {
  const split = splitConfig(definition);
  assert.equal(split.repository.collections.notes.path, 'data/notes');
  assert.equal(split.objects.anyOf[0]['x-collection'], 'notes');
  assert.equal(split.objects.anyOf[0].properties.text['x-content'], 'inline-markdown');
  assert.equal(split.objects.anyOf[0].properties.text['x-editor'], undefined);
  assert.equal(split.ui.collections.notes.fields['/properties/text']['x-editor'], 'inline-markdown');
  assert.equal(definition.anyOf[0].properties.text['x-editor'], 'inline-markdown');
});
test('import updates preserve settings and roundtrip resolved form metadata', () => {
  const root = mkdtempSync(join(tmpdir(), 'project-config-'));
  try {
    mkdirSync(join(root, '.editor'));
    writeProjectDefinition(root, definition);
    writeFileSync(join(root, 'settings.json'), '{"title":"Keep"}');
    writeFileSync(join(root, '.editor/settings.schema.json'), '{"type":"object","required":["title"]}');
    const manifestPath = join(root, '.editor/repository.json');
    const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
    manifest.documents.metadata = { schema: 'metadata.schema.json', files: ['a.json', 'nested/b.json'] };
    writeFileSync(manifestPath, JSON.stringify(manifest));
    mkdirSync(join(root, 'nested'));
    writeFileSync(join(root, 'metadata.schema.json'), '{"type":"object"}');
    writeFileSync(join(root, 'a.json'), '{"keep":"A"}');
    writeFileSync(join(root, 'nested/b.json'), '{"keep":"B"}');
    const settingsPath = join(root, manifest.collections.notes);
    const settings = JSON.parse(readFileSync(settingsPath, 'utf8'));
    settings.operations = ['read'];
    writeFileSync(settingsPath, JSON.stringify(settings));
    assert.equal(manifest.version, 2);
    assert.equal(manifest.objects, undefined);
    assert.equal(JSON.parse(readFileSync(join(root, settings.schema), 'utf8')).anyOf, undefined);

    const loaded = readProjectDefinition(root);
    assert.equal(loaded.anyOf[0].properties.text.title, 'Text');
    assert.equal(loaded.anyOf[0]['x-collection'].display, 'text');
    writeProjectDefinition(root, loaded);
    assert.deepEqual(JSON.parse(readFileSync(manifestPath, 'utf8')).documents, manifest.documents);
    assert.equal(readFileSync(join(root, 'a.json'), 'utf8'), '{"keep":"A"}');
    assert.equal(readFileSync(join(root, 'nested/b.json'), 'utf8'), '{"keep":"B"}');
    assert.equal(readFileSync(join(root, 'metadata.schema.json'), 'utf8'), '{"type":"object"}');
    assert.deepEqual(JSON.parse(readFileSync(settingsPath, 'utf8')).operations, ['read']);
    assert.deepEqual(JSON.parse(readFileSync(manifestPath, 'utf8')).collections.media, manifest.collections.media);
    assert.equal(JSON.parse(readFileSync(manifestPath, 'utf8')).settings, 'settings.json');
    assert.deepEqual(JSON.parse(readFileSync(join(root, 'settings.json'), 'utf8')), { title: 'Keep' });
    assert.deepEqual(JSON.parse(readFileSync(join(root, '.editor/settings.schema.json'), 'utf8')), { type: 'object', required: ['title'] });
  } finally { rmSync(root, { recursive: true, force: true }); }
});
