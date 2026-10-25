import {test} from 'node:test';
import assert from 'node:assert/strict';
import {JSDOM} from 'jsdom';
import {installPickerSearch} from '../src/controls/picker-search.ts';
function fixture(t) {
  const dom = new JSDOM('<form data-version="1"><input name="value.title" value="unsaved"><server-picker data-search-url="/choices"><input name="picker_query"><button data-picker-search>Search</button><div data-picker-options>Old</div></server-picker></form>',{url:'http://localhost'});
  t.after(()=>dom.window.close());
  const win=dom.window,doc=win.document,requests=[];
  win.fetch=(url,options)=>new Promise(resolve=>requests.push({url,options,resolve}));
  installPickerSearch(win);
  const input=doc.querySelector('[name=picker_query]');
  const query=value=>{input.value=value; input.dispatchEvent(new win.InputEvent('input',{bubbles:true}));};
  const reply=(n,value)=>requests[n].resolve({ok:true,text:async()=>`<div data-picker-options>${value}</div>`});
  const wait=ms=>new Promise(r=>setTimeout(r,ms));
  return {doc,input,requests,query,reply,wait};
}
test('typing filters without submitting a draft or moving focus; stale results are ignored',async t=>{
  const {doc,input,requests,query,reply,wait}=fixture(t);
  let submits=0;doc.addEventListener('submit',e=>{e.preventDefault();submits++;});
  input.focus(); query('be'); await wait(210); query('berlin'); await wait(210);
  assert.equal(requests[0].options.signal.aborted,true);
  assert.equal(requests[1].url,'/choices?q=berlin');
  reply(1,'Berlin');await wait(0);reply(0,'Stale');await wait(0);
  assert.equal(doc.querySelector('[data-picker-options]').textContent,'Berlin');
  assert.equal(doc.activeElement,input);assert.equal(submits,0);
  assert.equal(doc.querySelector('form').dataset.version,'1');
  assert.equal(doc.querySelector('[name="value.title"]').value,'unsaved');
});
test('clearing search and the Search button both work, including non-ASCII text',async t=>{
  const {doc,requests,query,reply,wait}=fixture(t);
  query('München');doc.querySelector('button').click();await wait(10);
  assert.equal(requests[0].url,'/choices?q=M%C3%BCnchen');reply(0,'München');await wait(0);
  query('');await wait(210);assert.equal(requests[1].url,'/choices?q=');reply(1,'All choices');await wait(0);
  assert.equal(doc.querySelector('[data-picker-options]').textContent,'All choices');
});
test('a concurrent version change triggers a fresh read instead of displaying stale selections',async t=>{
  const {doc,requests,query,reply,wait}=fixture(t);
  query('first');await wait(210);doc.querySelector('form').dataset.version='2';reply(0,'Unchecked');await wait(20);
  assert.equal(requests.length,2);reply(1,'Checked');await wait(0);
  assert.equal(doc.querySelector('[data-picker-options]').textContent,'Checked');
});
