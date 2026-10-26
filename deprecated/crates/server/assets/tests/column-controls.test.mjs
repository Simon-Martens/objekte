import { test } from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { installColumnControls } from '../src/controls/list-columns.ts';

function fixture(t) {
  const menu = id => `<details id="columns-${id}" class="column-picker"><summary>Columns</summary>
    <form><label class="column-option"><input type="checkbox" name="child.${id}.column.name"><span>Name</span></label></form></details>`;
  const dom = new JSDOM(menu('first') + menu('second'), { url: 'https://editor.test/record', pretendToBeVisual: true });
  t.after(() => dom.window.close());
  const { document } = dom.window;
  const opened = [];
  installColumnControls(document, element => { opened.push(element.id); element.open = true; });
  document.addEventListener('submit', event => event.preventDefault());
  return { window: dom.window, document, opened, finish(form) {
    document.dispatchEvent(new dom.window.CustomEvent('datastar-fetch', { detail: {type:'finished',el:form} }));
  } };
}
const frame = () => new Promise(resolve => setTimeout(resolve, 40));

test('after a table patch, reopen the changed chooser and restore its checkbox focus', async t => {
  const ui = fixture(t);
  const second = ui.document.getElementById('columns-second');
  const form = second.querySelector('form');
  const input = form.querySelector('input');
  input.focus();
  form.requestSubmit();
  const updated = second.cloneNode(true);
  second.replaceWith(updated);
  ui.finish(form);
  await frame();
  assert.deepEqual(ui.opened, ['columns-second']);
  assert.equal(ui.document.getElementById('columns-first').open, false);
  assert.equal(ui.document.activeElement, updated.querySelector('input'));
});

test('clicking a column label changes only its own table checkbox', t => {
  const ui = fixture(t);
  const inputs = [...ui.document.querySelectorAll('input')];
  const changes = [];
  ui.document.addEventListener('change', event => changes.push(event.target.name));
  ui.document.getElementById('columns-second').querySelector('span').click();
  assert.equal(inputs[0].checked, false);
  assert.equal(inputs[1].checked, true);
  assert.deepEqual(changes, ['child.second.column.name']);
});

test('navigation or dismissal prevents a finished request from reopening a chooser', async t => {
  for (const dismissal of ['navigate', 'escape']) {
    const ui = fixture(t);
    const form = ui.document.getElementById('columns-second').querySelector('form');
    form.requestSubmit();
    if (dismissal === 'navigate') ui.window.dispatchEvent(new ui.window.Event('navigate'));
    else ui.document.dispatchEvent(new ui.window.KeyboardEvent('keydown', {key:'Escape'}));
    ui.finish(form);
    await frame();
    assert.deepEqual(ui.opened, []);
  }
});
