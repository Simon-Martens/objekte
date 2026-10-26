import { test } from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { installAppearance } from '../src/controls/appearance.ts';

function setup(t) {
  const dom = new JSDOM(`<div id="ui-preferences" data-default-theme="dark" data-density="comfortable"></div>
    <button data-theme-choice="default"></button><button data-theme-choice="light"></button><button data-theme-choice="dark"></button>
    <select data-personal-theme><option>default</option><option>system</option><option>light</option><option>dark</option></select>
    <select data-personal-density><option>default</option><option>compact</option><option>comfortable</option></select>
    <button data-reset-appearance></button><span data-appearance-status></span>`, {url:'http://localhost'});
  t.after(() => dom.window.close());
  const win = dom.window, doc = win.document;
  const media = new win.EventTarget(); media.matches = false;
  win.matchMedia = () => media;
  installAppearance(doc, win);
  return {win, doc, media, root:doc.documentElement, choose(selector, value) {
    const input = doc.querySelector(selector); input.value = value; input.dispatchEvent(new win.Event('change', {bubbles:true}));
  }};
}

test('appearance follows project defaults, personal overrides persist, and reset restores defaults', t => {
  const ui = setup(t);
  assert.equal(ui.root.dataset.theme, 'dark');
  assert.equal(ui.root.dataset.density, 'comfortable');
  ui.choose('[data-personal-theme]', 'light');
  ui.choose('[data-personal-density]', 'compact');
  assert.equal(ui.root.dataset.theme, 'light');
  assert.equal(ui.root.dataset.density, 'compact');
  assert.equal(ui.win.localStorage.getItem('objekte-theme'), 'light');
  ui.doc.querySelector('[data-reset-appearance]').click();
  assert.equal(ui.root.dataset.theme, 'dark');
  assert.equal(ui.root.dataset.density, 'comfortable');
  assert.equal(ui.win.localStorage.getItem('objekte-theme'), null);
  assert.equal(ui.win.localStorage.getItem('objekte-density'), null);
  assert.equal(ui.doc.querySelector('[data-theme-choice=default]').getAttribute('aria-pressed'), 'true');
});

test('device theme remains independent of project theme and reacts to device changes', t => {
  const ui = setup(t);
  ui.choose('[data-personal-theme]', 'system');
  assert.equal(ui.root.dataset.theme, 'light');
  ui.media.matches = true; ui.media.dispatchEvent(new ui.win.Event('change'));
  assert.equal(ui.root.dataset.theme, 'dark');
  ui.doc.querySelector('[data-theme-choice=light]').click();
  ui.media.dispatchEvent(new ui.win.Event('change'));
  assert.equal(ui.root.dataset.theme, 'light');
});

test('saved project settings update defaults immediately without overwriting a personal choice', t => {
  const ui = setup(t);
  ui.choose('[data-personal-theme]', 'light');
  const defaults = ui.doc.getElementById('ui-preferences');
  defaults.dataset.defaultTheme = 'dark'; defaults.dataset.density = 'compact';
  ui.win.dispatchEvent(new ui.win.Event('preferences-change'));
  assert.equal(ui.root.dataset.theme, 'light');
  assert.equal(ui.root.dataset.density, 'compact');
  ui.win.localStorage.setItem('objekte-theme', 'dark');
  ui.win.dispatchEvent(new ui.win.StorageEvent('storage', {key:'objekte-theme',newValue:'dark'}));
  assert.equal(ui.root.dataset.theme, 'dark');
  assert.equal(ui.doc.querySelector('[data-personal-theme]').value, 'dark');
});
