import { test } from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { installProjectChooser, projectDraftKey } from '../src/controls/project-chooser.ts';

for (const accepted of [true, false]) test(`project selection respects navigation confirmation (${accepted})`, async t => {
  const dom = new JSDOM('<form data-project-chooser data-project-id="first"><select id="project-select"><option value="first">First</option><option value="second">Second</option></select></form>');
  t.after(() => dom.window.close());
  const { document, Event } = dom.window;
  const submissions = [];
  let resolve;
  installProjectChooser(document, () => new Promise(r => { resolve = r; }), form => submissions.push(form.querySelector('select').value));
  const select = document.querySelector('select');
  select.value = 'second';
  select.dispatchEvent(new Event('change', { bubbles: true }));
  assert.equal(select.value, 'first');
  assert.deepEqual(submissions, []);
  resolve(accepted);
  await Promise.resolve();
  assert.deepEqual(submissions, accepted ? ['second'] : []);
  assert.equal(select.value, accepted ? 'second' : 'first');
});

test('identical record URLs in different projects have independent draft keys', () => {
  assert.notEqual(projectDraftKey('first', '/collections/notes/records/one/edit'), projectDraftKey('second', '/collections/notes/records/one/edit'));
});
