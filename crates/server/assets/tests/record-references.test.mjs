import { test } from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { installRecordReferences } from '../src/controls/record-references.ts';

const flush = () => new Promise(resolve => setImmediate(resolve));
function setup(t, fetch) {
  const dom = new JSDOM('<main><h1>Record</h1><record-references data-url="/references"><p>Loading incoming references…</p></record-references></main>', { url: 'http://localhost' });
  t.after(() => dom.window.close());
  dom.window.fetch = fetch;
  installRecordReferences(dom.window);
  return dom.window;
}

test('record remains usable while backlinks load and pages render independently', async t => {
  let resolve;
  const calls = [];
  const win = setup(t, (url, init) => {
    calls.push([url, init]);
    return new Promise(done => { resolve = done; });
  });
  const panel = win.document.querySelector('record-references');
  assert.equal(win.document.querySelector('h1').textContent, 'Record');
  assert.equal(panel.getAttribute('aria-busy'), 'true');
  resolve(new Response('<p>First references</p><a data-reference-page href="/references?offset=100">Next</a>'));
  await flush();
  assert.equal(panel.hasAttribute('aria-busy'), false);
  panel.querySelector('a').click();
  assert.equal(calls.length, 2);
  assert.equal(calls[1][0], 'http://localhost/references?offset=100');
  resolve(new Response('<p>Second references</p>'));
  await flush();
  assert.equal(panel.textContent, 'Second references');
});

test('leaving a record aborts its request and ignores late results', async t => {
  let resolve, signal;
  const win = setup(t, (_url, init) => {
    signal = init.signal;
    return new Promise(done => { resolve = done; });
  });
  const panel = win.document.querySelector('record-references');
  panel.remove();
  assert.equal(signal.aborted, true);
  resolve(new Response('<p>Obsolete references</p>'));
  await flush();
  assert.ok(!panel.textContent.includes('Obsolete'));
  assert.equal(win.document.querySelector('h1').textContent, 'Record');
});

for (const response of [new Response('failed', { status: 500 }), { ok: true, redirected: true }]) {
  test(`failed references offer a retry (${response.status ?? 'redirect'})`, async t => {
    let calls = 0;
    const win = setup(t, async () => ++calls === 1 ? response : new Response('<p>No incoming references.</p>'));
    await flush();
    win.document.querySelector('[data-reference-retry]').click();
    await flush();
    assert.equal(calls, 2);
    assert.equal(win.document.querySelector('record-references').textContent, 'No incoming references.');
  });
}

test('a reused panel follows its new record and ignores an older response', async t => {
  const requests = [];
  const win = setup(t, (url, { signal }) => new Promise(resolve => requests.push({ url, signal, resolve })));
  const panel = win.document.querySelector('record-references');
  panel.dataset.url = '/second/references';
  await flush();
  assert.equal(requests.length, 2);
  assert.ok(requests[0].signal.aborted);
  requests[1].resolve(new Response('<p>Second record references</p>'));
  await flush();
  requests[0].resolve(new Response('<p>Obsolete first record references</p>'));
  await flush();
  assert.equal(panel.textContent, 'Second record references');
});
