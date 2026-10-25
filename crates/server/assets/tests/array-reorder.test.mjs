import { test } from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { installArrayReorder } from '../src/controls/array-reorder.ts';
function setup(t, next = '') {
  const dom = new JSDOM(`<form id="record-form" data-version="4"><input name="version" value="4"><input name="value.unsent" value="unfinished"><div data-array="/array/actions" data-array-next="${next}">${['a','b','c'].map((id,i) => `<div data-array-item="${id}"><button type="button" data-array-handle data-up="${i ? '/array/actions/up?item='+id : ''}" data-down="/array/actions/down?item=${id}">Move</button></div>`).join('')}</div></form>`, {url:'http://localhost'});
  t.after(() => dom.window.close());
  const win = dom.window, doc = win.document;
  const items = [...doc.querySelectorAll('[data-array-item]')];
  items.forEach((row,i) => row.getBoundingClientRect = () => ({top:100+i*40,bottom:140+i*40,left:0,right:300,width:300,height:40}));
  const sent = [];
  doc.querySelector('form').requestSubmit = button => sent.push({url:button.getAttribute('formaction'), form:Object.fromEntries(new win.FormData(button.form))});
  installArrayReorder(win);
  const event = (target, type, x, y) => target.dispatchEvent(new win.MouseEvent(type,{bubbles:true,clientX:x,clientY:y,button:0}));
  return {win,doc,items,sent,event};
}
test('drag uses stable destinations and submits unsent input with draft version', t => {
  const {doc,items,sent,event} = setup(t);
  event(items[2].firstChild,'pointerdown',15,190);
  event(doc,'pointermove',15,105);
  assert.ok(items[0].classList.contains('array-drop-before'));
  event(doc,'pointerup',15,105);
  assert.deepEqual(sent,[{url:'/array/actions/move?item=c',form:{version:'4','value.unsent':'unfinished',array_before:'a'}}]);
  assert.equal(doc.querySelector('.array-dragging'),null);
});
test('dropping after the last visible row uses the next page boundary', t => {
  const {doc,items,sent,event} = setup(t,'next-page');
  event(items[0].firstChild,'pointerdown',15,110);
  event(doc,'pointermove',15,215);
  event(doc,'pointerup',15,215);
  assert.equal(sent[0].form.array_before,'next-page');
});
test('keyboard arrows reorder while Escape and dropping outside cancel', t => {
  const {win,doc,items,sent,event} = setup(t);
  items[1].firstChild.dispatchEvent(new win.KeyboardEvent('keydown',{key:'ArrowUp',bubbles:true}));
  assert.equal(sent[0].url,'/array/actions/up?item=b');
  event(items[0].firstChild,'pointerdown',15,110);
  event(doc,'pointermove',15,210);
  doc.dispatchEvent(new win.KeyboardEvent('keydown',{key:'Escape',bubbles:true}));
  event(doc,'pointerup',15,210);
  assert.equal(sent.length,1);
  event(items[0].firstChild,'pointerdown',15,110);
  event(doc,'pointermove',350,210);
  event(doc,'pointerup',350,210);
  assert.equal(sent.length,1);
});

test('regular table moves, keyboard reorder and removal update form state immediately', t => {
  const {win,doc,items,sent,event}=setup(t);
  const array=doc.querySelector('[data-array]');
  array.dataset.structureArray='rows';array.dataset.arrayOrder='["a","b","c"]';
  array.dataset.reorderLabel='Reorder item {number}';
  items.forEach(row=>{const button=doc.createElement('button');button.type='submit';button.dataset.removeArrayItem='';row.append(button);});
  items[2].firstChild.dispatchEvent(new win.KeyboardEvent('keydown',{key:'ArrowUp',bubbles:true}));
  assert.deepEqual([...array.querySelectorAll('[data-array-item]')].map(row=>row.dataset.arrayItem),['a','c','b']);
  assert.equal(doc.querySelector('[name="structure.rows"]').value,'["a","c","b"]');
  assert.equal(doc.querySelector('form').dataset.version,'4');assert.equal(sent.length,0);
  array.querySelector('[data-array-item="a"] [data-remove-array-item]').click();
  assert.equal(doc.querySelector('[name="structure.rows"]').value,'["c","b"]');
  assert.equal(sent.length,0);
  assert.equal(new win.FormData(doc.querySelector('form')).get('value.unsent'),'unfinished');
  event(items[1].firstChild,'pointerdown',15,190);
  event(doc,'pointermove',15,105);
  event(doc,'pointerup',15,105);
  assert.equal(sent.length,0);
});

test('paged table retains hidden item identities when visible rows are rearranged', t => {
  const {win,doc,items,sent}=setup(t,'d');
  const array=doc.querySelector('[data-array]');
  array.dataset.structureArray='rows';array.dataset.arrayOrder='["a","b","c","d","e"]';
  items[2].firstChild.dispatchEvent(new win.KeyboardEvent('keydown',{key:'ArrowUp',bubbles:true}));
  assert.equal(doc.querySelector('[name="structure.rows"]').value,'["a","c","b","d","e"]');
  assert.equal(sent.length,0);
});
