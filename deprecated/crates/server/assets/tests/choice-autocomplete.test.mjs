import {test} from 'node:test';
import assert from 'node:assert/strict';
import {JSDOM} from 'jsdom';
import {installChoiceAutocomplete} from '../src/controls/choice-autocomplete.ts';
import {installChoiceValues} from '../src/controls/choice-values.ts';
import {installArrayReorder} from '../src/controls/array-reorder.ts';
function fixture(t) {
  const dom = new JSDOM('<form data-version="1"><input name="value.title" value="unsaved"><div data-select-anchor data-local-choices="places" data-array="/places/actions" aria-label="Places"><input id="input-places" data-choice-input data-search-url="/choices" data-no-matches="No matches." aria-controls="suggestions-places"><button formaction="/multiple">Browse</button></div></form>',{url:'http://localhost'});
  t.after(()=>dom.window.close());
  const win=dom.window,doc=win.document,requests=[];
  win.fetch=(url,options)=>new Promise(resolve=>requests.push({url,options,resolve}));
  installChoiceAutocomplete(win,installChoiceValues(win)); installArrayReorder(win);
  const input=doc.querySelector('[data-choice-input]');input.focus();
  const query=value=>{input.value=value;input.dispatchEvent(new win.InputEvent('input',{bubbles:true}));};
  const key=value=>input.dispatchEvent(new win.KeyboardEvent('keydown',{key:value,bubbles:true,cancelable:true}));
  const reply=(n,labels)=>requests[n].resolve({ok:true,text:async()=>`<div data-picker-options>${labels.map(label=>`<button name="choice" value='"${label}"' aria-checked="${label==='Selected'}" formaction="/toggle-choice"><span>${label}</span></button>`).join('')}</div>`});
  const wait=ms=>new Promise(r=>setTimeout(r,ms));
  return {win,doc,input,requests,query,key,reply,wait};
}
test('autocomplete ignores stale responses and never submits unmatched query text',async t=>{
  const {win,doc,input,requests,query,key,reply,wait}=fixture(t);
  let submits=0;doc.addEventListener('submit',e=>{e.preventDefault();submits++;});
  query('Ber');await wait(210);query('unknown');key('Enter');await wait(210);
  assert.equal(requests[0].options.signal.aborted,true);
  reply(1,[]);await wait(0);reply(0,['Berlin']);await wait(0);key('Enter');
  assert.equal(submits,0);assert.equal(doc.querySelector('[role=status]').textContent,'No matches.');
  assert.equal(doc.activeElement,input);assert.equal(doc.querySelector('form').dataset.version,'1');
  assert.deepEqual([...new win.FormData(doc.querySelector('form')).entries()],[['value.title','unsaved']]);
});
test('keyboard adds pills synchronously without submission, version change or a network mutation',async t=>{
  const {doc,input,query,key,reply,wait}=fixture(t);
  query('B');await wait(210);reply(0,['Berlin','Bonn','Selected']);await wait(0);
  assert.equal(doc.querySelectorAll('[role=option]').length,3);
  key('ArrowDown');assert.equal(doc.getElementById(input.getAttribute('aria-activedescendant')).textContent,'Bonn');
  let submits=0;doc.addEventListener('submit',e=>{e.preventDefault();submits++;});
  key('Enter');
  assert.equal(doc.querySelector('.array-choice-label').textContent,'Bonn');
  assert.equal(doc.querySelector('[data-choice-values]').value,'["Bonn"]');
  assert.equal(submits,0);assert.equal(doc.querySelector('form').dataset.version,'1');
  assert.equal(input.value,'');assert.equal(doc.activeElement,input);assert.equal(input.getAttribute('aria-expanded'),'false');
});
test('new query clears old selection immediately; Escape, Tab and Browse dismiss suggestions',async t=>{
  const {doc,input,query,key,reply,wait}=fixture(t);
  let submits=0;doc.addEventListener('submit',e=>{e.preventDefault();submits++;});
  query('Berlin');await wait(210);reply(0,['Berlin']);await wait(0);
  query('Elsewhere');key('Enter');assert.equal(submits,0);assert.equal(doc.querySelector('[role=option]'),null);
  key('Escape');await wait(210);assert.equal(doc.querySelector('[role=listbox]'),null);
  key('ArrowDown');await wait(10);reply(1,['Berlin']);await wait(0);key('Tab');
  assert.equal(input.getAttribute('aria-expanded'),'false');doc.querySelector('button').click();assert.equal(submits,1);
});

test('rapid selections, removal, and handle reordering are local ordered form input',async t=>{
  const {win,doc,input,requests,query,key,reply,wait}=fixture(t);
  let submits=0;doc.addEventListener('submit',e=>{e.preventDefault();submits++;});
  query('Ber');await wait(210);reply(0,['Berlin']);await wait(0);key('Enter');
  query('Bon');await wait(210);reply(1,['Bonn']);await wait(0);doc.querySelector('[role=option]').click();
  assert.equal(requests.length,2);assert.equal(doc.querySelector('[data-choice-values]').value,'["Berlin","Bonn"]');
  const handles=doc.querySelectorAll('[data-array-handle]');
  handles[1].dispatchEvent(new win.KeyboardEvent('keydown',{key:'ArrowUp',bubbles:true}));
  assert.equal(doc.querySelector('[data-choice-values]').value,'["Bonn","Berlin"]');
  doc.querySelector('[data-remove-choice]').click();
  assert.equal(doc.querySelector('[data-choice-values]').value,'["Berlin"]');assert.equal(submits,0);
  assert.equal(doc.activeElement,input);
  query('Ber');await wait(210);reply(2,['Berlin']);await wait(0);assert.equal(doc.querySelector('[role=option]'),null);
});
test('Browse selections update chips, checkboxes and count locally, including refreshed results',async t=>{
  const {doc,wait}=fixture(t);
  const field=doc.querySelector('[data-local-choices]');field.parentElement.id='field-places';
  const picker=doc.createElement('server-picker');picker.dataset.popover='';picker.dataset.returnField='places';
  picker.innerHTML=`<button name="choice" value='"Berlin"' aria-checked="false"><i></i><span>Berlin</span></button><span data-selection-count="{count} selected"></span>`;
  doc.querySelector('form').append(picker);
  let submits=0;doc.addEventListener('submit',e=>{e.preventDefault();submits++;});
  picker.querySelector('button').click();await wait(0);
  assert.equal(doc.querySelector('[data-choice-values]').value,'["Berlin"]');
  assert.equal(picker.querySelector('button').getAttribute('aria-checked'),'true');
  assert.equal(picker.querySelector('[data-selection-count]').textContent,'1 selected');
  const replacement=picker.querySelector('button').cloneNode(true);replacement.setAttribute('aria-checked','false');picker.querySelector('button').replaceWith(replacement);await wait(0);
  assert.equal(replacement.getAttribute('aria-checked'),'true');replacement.click();
  assert.equal(doc.querySelector('[data-choice-values]').value,'[]');assert.equal(submits,0);
});
