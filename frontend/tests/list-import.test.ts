import test from 'node:test';
import assert from 'node:assert/strict';
import { convertListRecord, listCollectionSchema } from '../scripts/list-content.mjs';

test('list import extracts Markdown, notes, metadata, and media links', () => {
  const value = convertListRecord('posthume_ausgaben', {
    source: 'source.html', Jahr: 1819, Sort: 1,
    body: '<p class="Einzug"><b>Titel</b> &lt;x-x&gt; <a href="/static/book/a.pdf">PDF</a></p><p class="Kommentar">Eine <i>Notiz</i>.</p>',
    label: 'old generated label',
  });
  assert.deepEqual(value, {
    source: 'source.html', Jahr: 1819, Sort: 1,
    Eintrag: '**Titel** \\<x-x> [PDF](/api/media/file?path=book%2Fa.pdf)',
    Kommentare: ['Eine _Notiz_.'],
  });
});

test('print records split stable citation and physical description patterns', () => {
  const journal = convertListRecord('unselbstaendige_drucke', {
    source: 'journal.html', Werk: 'Journal', Sort: 1,
    body: '<p class="Einzug"><i>Der Titel</i><br> In: Journal, S. 1. <a href="https://example.org/ ">Online</a></p>',
  });
  assert.equal(journal.Titelzeile, '_Der Titel_');
  assert.equal(journal.Fundstelle, 'In: Journal, S. 1. [Online](https://example.org/)');
  const journalSchema = listCollectionSchema('unselbstaendige_drucke', 'Journals', [{ id: 'one', value: journal }]);
  const properties = journalSchema.properties as Record<string, { 'x-editor'?: string }>;
  assert.equal(properties.Titelzeile['x-editor'], 'inline-markdown');
  assert.equal(properties.Fundstelle['x-editor'], 'markdown');
  assert.equal('body' in properties, false);
  assert.equal('kind' in properties, false);

  const book = convertListRecord('selbststaendige_drucke', {
    source: 'book.html', Jahr: 1770, Sort: 1,
    body: '<p class="Einzug">Titelblatt</p><p class="Kommentar">100 S. – Satzspiegel: 10 cm</p><p class="Kommentar">Zusatz.</p><p class="Kommentar">Exemplare: Berlin.</p>',
  });
  assert.equal(book.Titelblatt, 'Titelblatt');
  assert.equal(book.Kollation, '100 S. – Satzspiegel: 10 cm');
  assert.equal(book.Exemplare, 'Berlin.');
  assert.deepEqual(book.Anmerkungen, ['Zusatz.']);
});

test('performance titles use one field and unusual empty records remain importable', () => {
  const performance = convertListRecord('theaterauffuehrungen', {
    source: 'performance.html', Jahr: 1950,
    Name: 'Der Hofmeister', body: '<p class="Einzug">Aufführung.</p>',
  });
  assert.equal(performance.Werk, 'Der Hofmeister');
  assert.equal(performance.Name, undefined);
  const empty = convertListRecord('sekundaerliteratur', {
    source: 'empty.html', Jahr: 2015, Autor: 'Weber', Sort: 2, body: '',
  });
  assert.equal(empty.Eintrag, '');
  const noteOnly = convertListRecord('unselbstaendige_drucke', {
    source: 'note.html', Werk: 'Iris', Sort: 1,
    body: '<p class="Kommentar">Only a note.</p>',
  });
  assert.deepEqual(noteOnly.Kommentare, ['Only a note.']);
  assert.equal(noteOnly.Titelzeile, undefined);
});
