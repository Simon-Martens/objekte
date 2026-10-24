import { test } from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { installSearchControls } from '../src/controls/regex-search.ts';

const flush = () => new Promise(resolve => setTimeout(resolve, 10));
function fixture(t, validate = async () => '') {
  const dom = new JSDOM(`<section class="table-panel"><form data-regex-search>
    <input type="search" name="q" aria-invalid="false">
    <input type="hidden" name="regex" value="0"><input type="hidden" name="case_sensitive" value="0">
    <button type="button" data-case-toggle aria-pressed="false"><span>Aa</span></button>
    <button type="button" data-regex-toggle aria-pressed="false"><span>.*</span></button>
    <button>Search</button></form><span id="search-pattern-error" role="status"></span></section>`);
  t.after(() => dom.window.close());
  const { document } = dom.window;
  installSearchControls(document, validate);
  const form = document.querySelector('form');
  const input = form.elements.q;
  const submissions = [];
  // Same event phase as Datastar's form handler. Invalid/pending patterns must
  // never reach it, and validated submissions must reach it exactly once.
  form.addEventListener('submit', event => {
    event.preventDefault();
    submissions.push(Object.fromEntries(new dom.window.FormData(form)));
  });
  return {
    form, input, document, submissions,
    error: document.querySelector('[role=status]'),
    regex: document.querySelector('[data-regex-toggle]'),
    matchCase: document.querySelector('[data-case-toggle]'),
    type(value) {
      input.value = value;
      input.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
    },
  };
}

test('literal search toggles case, keeps focus, and submits once without loading WASM', async t => {
  const ui = fixture(t, () => assert.fail('literal search should not load the regex compiler'));
  ui.type('Ü.a');
  ui.matchCase.querySelector('span').click();
  assert.equal(ui.matchCase.getAttribute('aria-pressed'), 'true');
  assert.equal(ui.document.activeElement, ui.input);
  ui.form.requestSubmit();
  await flush();
  assert.deepEqual(ui.submissions, [{ q: 'Ü.a', regex: '0', case_sensitive: '1' }]);
  ui.matchCase.click();
  ui.form.requestSubmit();
  await flush();
  assert.equal(ui.matchCase.getAttribute('aria-pressed'), 'false');
  assert.deepEqual(ui.submissions[1], { q: 'Ü.a', regex: '0', case_sensitive: '0' });
});

test('regex and case toggles are independent and validate the exact selected options', async t => {
  const calls = [];
  const ui = fixture(t, async (...args) => { calls.push(args); return ''; });
  ui.type('(?i)^über$');
  ui.regex.click();
  await flush();
  ui.matchCase.click();
  await flush();
  ui.form.requestSubmit();
  await flush();
  assert.deepEqual(calls, [['(?i)^über$', false], ['(?i)^über$', true]]);
  assert.deepEqual(ui.submissions, [{ q: '(?i)^über$', regex: '1', case_sensitive: '1' }]);
});

test('an immediate first submission resumes after the original event turn', async t => {
  const ui = fixture(t);
  ui.type('Hello');
  ui.form.requestSubmit();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(ui.submissions.length, 0);
  await flush();
  assert.deepEqual(ui.submissions, [{ q: 'Hello', regex: '0', case_sensitive: '0' }]);
});

test('typing validates before submission; invalid input blocks submission and editing recovers', async t => {
  const ui = fixture(t, async pattern => pattern === '[' ? 'Unclosed character class' : '');
  ui.regex.click();
  await flush();
  ui.type('[');
  await new Promise(resolve => setTimeout(resolve, 160));
  assert.equal(ui.input.getAttribute('aria-invalid'), 'true');
  assert.equal(ui.input.validationMessage, 'Unclosed character class');
  assert.equal(ui.error.textContent, 'Unclosed character class');
  ui.form.requestSubmit();
  await flush();
  assert.equal(ui.submissions.length, 0);
  ui.type('^fixed$');
  assert.equal(ui.input.getAttribute('aria-invalid'), 'false');
  ui.form.requestSubmit();
  await flush();
  assert.equal(ui.submissions.length, 1);
  assert.equal(ui.submissions[0].q, '^fixed$');
  assert.equal(ui.error.textContent, '');
});

test('switching an invalid regex to literal mode clears the error and submits the literal', async t => {
  const ui = fixture(t, async () => 'Unclosed character class');
  ui.type('[');
  ui.regex.click();
  await flush();
  assert.equal(ui.input.validity.valid, false);
  ui.regex.click();
  ui.form.requestSubmit();
  await flush();
  assert.equal(ui.input.validity.valid, true);
  assert.equal(ui.error.textContent, '');
  assert.deepEqual(ui.submissions, [{ q: '[', regex: '0', case_sensitive: '0' }]);
});

test('submission waits for validation and ignores older out-of-order results', async t => {
  const pending = [];
  const ui = fixture(t, () => new Promise(resolve => pending.push(resolve)));
  ui.type('old');
  ui.regex.click();
  ui.form.requestSubmit();
  assert.equal(ui.submissions.length, 0);
  ui.type('new');
  ui.form.requestSubmit();
  pending[2]('');
  await flush();
  assert.equal(ui.submissions.length, 1);
  assert.equal(ui.submissions[0].q, 'new');
  pending[0]('Old invalid pattern');
  pending[1]('');
  await flush();
  assert.equal(ui.submissions.length, 1);
  assert.equal(ui.input.getAttribute('aria-invalid'), 'false');
  assert.equal(ui.error.textContent, '');
});

test('changing case during pending submission cancels that submission', async t => {
  const pending = [];
  const ui = fixture(t, () => new Promise(resolve => pending.push(resolve)));
  ui.regex.click();
  ui.type('pattern');
  ui.form.requestSubmit();
  ui.matchCase.click();
  pending[1]('');
  pending[0]('');
  await flush();
  assert.equal(ui.submissions.length, 0);
  pending[2]('');
  await flush();
  ui.form.requestSubmit();
  assert.equal(ui.submissions.length, 1);
  assert.equal(ui.submissions[0].case_sensitive, '1');
});

for (const navigation of ['replaced', 'morphed']) {
  test(`pending validation cannot submit or mark a form after it is ${navigation}`, async t => {
    const pending = [];
    const ui = fixture(t, () => new Promise(resolve => pending.push(resolve)));
    ui.regex.click();
    ui.type('old');
    ui.form.requestSubmit();
    if (navigation === 'replaced') ui.form.remove();
    else ui.input.value = 'navigated'; // DOM morphing does not dispatch input.
    pending[0]('');
    pending[1]('Old invalid pattern');
    await flush();
    assert.equal(ui.submissions.length, 0);
    assert.equal(ui.error.textContent, '');
    assert.equal(ui.input.getAttribute('aria-invalid'), 'false');
  });
}

test('WASM load failure permits server validation without looping or blocking the form', async t => {
  const ui = fixture(t, async () => { throw new Error('offline'); });
  ui.type('pattern');
  ui.regex.click();
  ui.form.requestSubmit();
  await flush();
  assert.equal(ui.submissions.length, 1);
  assert.equal(ui.input.validity.valid, true);
  assert.match(ui.error.textContent, /validate it on the server/);
});
