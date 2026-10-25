import { test } from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { installRecordReferences } from '../src/controls/record-references.ts';
import { installSearchControls } from '../src/controls/regex-search.ts';

const flush = () => new Promise(resolve => setImmediate(resolve));
const rows = text => `<section><h2 tabindex="-1">Entries</h2><p>${text}</p><button data-reference-refresh>Refresh</button></section>`;
const table = (name, body = rows('First records')) => `<record-reference-table id="references-${name}" data-url="/references?source=${name}">${body}</record-reference-table>`;
function setup(t, fetch) {
  const dom = new JSDOM('<main><h1>Record</h1><record-references hidden data-url="/references"></record-references></main>', { url: 'http://localhost' });
  t.after(() => dom.window.close());
  dom.window.fetch = fetch;
  installRecordReferences(dom.window);
  return dom.window;
}

test('record stays usable while checking and empty link groups never appear', async t => {
  let resolve;
  const win = setup(t, () => new Promise(done => { resolve = done; }));
  const panel = win.document.querySelector('record-references');
  assert.equal(win.document.querySelector('h1').textContent, 'Record');
  assert.equal(panel.hidden, true);
  assert.equal(panel.getAttribute('aria-busy'), 'true');
  resolve(new Response(''));
  await flush();
  assert.equal(panel.hidden, true);
  assert.equal(panel.textContent, '');
  assert.equal(panel.hasAttribute('aria-busy'), false);
});

test('related tables reveal only with links and page independently without navigating', async t => {
  const calls = [];
  const win = setup(t, async url => {
    calls.push(url);
    return new Response(calls.length === 1
      ? table('entries', rows('First entries') + '<a data-reference-query data-reference-page href="/references?source=entries&offset=25">Next</a>') + table('series', rows('Series unchanged'))
      : rows('Next entries'));
  });
  await flush();
  const root = win.document.querySelector('record-references');
  const entries = win.document.querySelector('#references-entries');
  let scrolled = false, navigated = false;
  entries.scrollIntoView = () => { scrolled = true; };
  win.document.addEventListener('click', () => { navigated = true; });
  assert.equal(root.hidden, false);
  assert.equal(calls.length, 1); // Rendered child tables do not fetch again.
  entries.querySelector('a').click();
  await flush();
  assert.equal(calls.length, 2);
  assert.equal(calls[1], 'http://localhost/references?source=entries&offset=25');
  assert.ok(entries.textContent.includes('Next entries'));
  assert.ok(win.document.querySelector('#references-series').textContent.includes('Series unchanged'));
  assert.equal(scrolled, true);
  assert.equal(navigated, false);
  assert.equal(win.document.activeElement.tagName, 'H2');
});

test('leaving a record aborts its request and ignores late results', async t => {
  let resolve, signal;
  const win = setup(t, (_url, init) => { signal = init.signal; return new Promise(done => { resolve = done; }); });
  const panel = win.document.querySelector('record-references');
  panel.remove();
  assert.equal(signal.aborted, true);
  resolve(new Response(table('entries')));
  await flush();
  assert.equal(panel.textContent, '');
});

for (const response of [new Response('failed', { status: 500 }), { ok: true, redirected: true }]) {
  test(`failed table refresh keeps existing rows and offers retry (${response.status ?? 'redirect'})`, async t => {
    let calls = 0;
    const win = setup(t, async () => {
      calls++;
      return calls === 1 ? new Response(table('entries', rows('Original entries'))) : calls === 2 ? response : new Response(rows('Updated entries'));
    });
    await flush();
    const panel = win.document.querySelector('record-reference-table');
    panel.querySelector('[data-reference-refresh]').click();
    await flush();
    assert.ok(panel.textContent.includes('Original entries'));
    assert.equal(panel.hidden, false);
    panel.querySelector('[data-reference-retry]').click();
    await flush();
    assert.equal(calls, 3);
    assert.ok(panel.textContent.includes('Updated entries'));
    assert.equal(panel.querySelector('.reference-state'), null);
  });
}

test('a reused loader hides old results and ignores an older response', async t => {
  const requests = [];
  const win = setup(t, (url, {signal}) => new Promise(resolve => requests.push({url, signal, resolve})));
  const panel = win.document.querySelector('record-references');
  panel.dataset.url = '/second/references';
  await flush();
  assert.equal(requests.length, 2);
  assert.ok(requests[0].signal.aborted);
  requests[1].resolve(new Response(''));
  await flush();
  requests[0].resolve(new Response(table('entries')));
  await flush();
  assert.equal(panel.hidden, true);
  assert.equal(panel.textContent, '');
});

test('removing the last link hides its table and removing every group hides the container', async t => {
  let calls = 0;
  const win = setup(t, async () => new Response(++calls === 1 ? table('entries') + table('series') : ''));
  await flush();
  const root = win.document.querySelector('record-references');
  const entries = win.document.querySelector('#references-entries');
  entries.querySelector('button').click();
  await flush();
  assert.equal(entries.hidden, true);
  assert.equal(root.hidden, false);
  win.document.querySelector('#references-series button').click();
  await flush();
  assert.equal(root.hidden, true);
});

test('a failed initial check does not reveal an unconfirmed table', async t => {
  const win = setup(t, async () => new Response('failed', {status:500}));
  await flush();
  assert.equal(win.document.querySelector('record-references').hidden, true);
});

test('table forms preserve their source and wait for regex validation', async t => {
  const calls = [];
  const form = '<form data-reference-query data-regex-search action="/references"><input type="hidden" name="source" value="entries"><input type="search" data-search-query name="q" value="[" aria-describedby="pattern-error"><input data-search-regex name="regex" value="1"><input data-search-case name="case_sensitive" value="0"><button>Search</button></form><span id="pattern-error"></span>';
  const win = setup(t, async url => { calls.push(url); return new Response(calls.length === 1 ? table('entries', form) : rows('Search result')); });
  installSearchControls(win.document, async pattern => pattern === '[' ? 'Invalid pattern' : '');
  await flush();
  const search = win.document.querySelector('form');
  search.requestSubmit();
  await flush();
  assert.equal(calls.length, 1);
  assert.equal(win.document.querySelector('#pattern-error').textContent, 'Invalid pattern');
  const input = search.querySelector('[data-search-query]');
  input.value = '^Alpha';
  input.dispatchEvent(new win.Event('input', {bubbles:true}));
  search.requestSubmit();
  await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(calls.length, 2);
  const query = new URL(calls[1]).searchParams;
  assert.equal(query.get('source'), 'entries');
  assert.equal(query.get('q'), '^Alpha');
  assert.equal(query.get('regex'), '1');
});
