import { test } from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { installEditTableSearch } from '../src/controls/edit-table-search.ts';

test('Enter submits the embedded edit table search action', t => {
  const dom = new JSDOM('<form><button formaction="/fields/x/actions/remove">Remove</button><div class="embedded-edit-search"><input type="search"><button formaction="/fields/x/actions/table-search">Search</button></div></form>');
  t.after(() => dom.window.close());
  installEditTableSearch(dom.window.document);
  const form = dom.window.document.querySelector('form');
  const input = form.querySelector('input');
  let submitter;
  form.requestSubmit = button => { submitter = button; };
  const event = new dom.window.KeyboardEvent('keydown', {key:'Enter', bubbles:true, cancelable:true});
  input.dispatchEvent(event);
  assert.equal(event.defaultPrevented, true);
  assert.equal(submitter.getAttribute('formaction'), '/fields/x/actions/table-search');
});
