import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
// @ts-ignore Import scripts are plain JavaScript.
import { convertHandschriften } from '../scripts/handschriften.mjs';
// @ts-ignore Import scripts are plain JavaScript.
import { splitConfig } from '../scripts/project-config.mjs';

test('manuscript import preserves descriptions and builds both kinds of parent-owned lists', () => {
  const root=mkdtempSync(join(tmpdir(),'hierarchy-import-'));
  try {
    const directory=join(root,'src/dynamic/lists/handschriften');mkdirSync(directory,{recursive:true});
    writeFileSync(join(directory,'test.html'),'---\nOrt: Österreich\nShort: Wien\n---\n<div class="information">Place note</div><div class="handschrift">Direct note</div><h2 class="ueberschrift-gruppe">Group</h2><div class="information">Group note</div><div class="handschrift">Grouped note</div>');
    const result=convertHandschriften(root);
    assert.equal(result.records.length,2);
    const place=result.places[0], group=result.subcollections[0];
    assert.deepEqual(place.value.sammlungen,[group.id]);
    assert.deepEqual(place.value.handschriften,[result.records[0].id]);
    assert.deepEqual(group.value.handschriften,[result.records[1].id]);
    assert.equal(place.value.description,'Place note');assert.equal(group.value.description,'Group note');
    for(const {value} of [...result.subcollections,...result.records]) {assert.equal(value.Ort,undefined);assert.equal(value.Sammlung,undefined);}
    const split=splitConfig({'x-project':{media:'media'},anyOf:[result.placeSchema,result.subcollectionSchema,result.schema]});
    assert.equal(split.ui.collections.orte.sidebarLabel,'Handschriften');assert.equal(split.ui.collections.orte.allCollection,'handschriften');
    assert.equal(split.objects.anyOf[0].properties.handschriften['x-children'],true);
    assert.equal(split.objects.anyOf[0].properties.handschriften.title,undefined);
  } finally {rmSync(root,{recursive:true,force:true});}
});
