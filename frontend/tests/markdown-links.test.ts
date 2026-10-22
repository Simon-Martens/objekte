import test from 'node:test';
import assert from 'node:assert/strict';
import { linkAtSelection } from '../src/lib/markdown-links';

test('finds the whole link when the cursor is in its label or destination', () => {
  const markdown = 'Before [old label](https://example.org/old) after';
  const expected = {
    start: markdown.indexOf('['), end: markdown.indexOf(')') + 1,
    label: 'old label', destination: 'https://example.org/old', image: false,
  };
  assert.deepEqual(linkAtSelection(markdown, markdown.indexOf('label'), markdown.indexOf('label')), expected);
  assert.deepEqual(linkAtSelection(markdown, markdown.indexOf('example'), markdown.indexOf('example')), expected);
});

test('recognizes image links with escaped labels and balanced destination parentheses', () => {
  const markdown = '![an \\[image\\]](/api/media/file?path=book%20(1).jpg)';
  assert.deepEqual(linkAtSelection(markdown, 4, 4), {
    start: 0, end: markdown.length, label: 'an [image]',
    destination: '/api/media/file?path=book%20(1).jpg', image: true,
  });
});

test('ignores escaped link syntax and unrelated selections', () => {
  const markdown = '\\[literal](elsewhere) and [real](https://example.org)';
  assert.equal(linkAtSelection(markdown, 4, 4), null);
  assert.equal(linkAtSelection(markdown, 20, 20), null);
  assert.equal(linkAtSelection(markdown, 0, 4), null);
  assert.equal(linkAtSelection(markdown, markdown.indexOf('real'), markdown.indexOf('real'))?.label, 'real');
});
