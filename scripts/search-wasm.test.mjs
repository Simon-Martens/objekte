import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

test('browser compiler accepts Rust syntax, rejects invalid patterns, and reuses memory', async () => {
  const bytes = await readFile(new URL('../crates/server/assets/src/generated/search.wasm', import.meta.url));
  const { instance: { exports: wasm } } = await WebAssembly.instantiate(bytes);
  const validate = (pattern, caseSensitive) => {
    const bytes = new TextEncoder().encode(pattern);
    const capacity = Math.max(bytes.length, 8192);
    const pointer = wasm.search_alloc(capacity);
    try {
      new Uint8Array(wasm.memory.buffer, pointer, bytes.length).set(bytes);
      const length = wasm.search_validate(pointer, bytes.length, capacity, Number(caseSensitive));
      return new TextDecoder().decode(new Uint8Array(wasm.memory.buffer, pointer, length));
    } finally { wasm.search_free(pointer, capacity); }
  };
  for (let i = 0; i < 50; i++) {
    const caseSensitive = i % 2 === 1;
    for (const pattern of ['', '^über.*$', '(?i)Hello', '\\p{Greek}+', '[a-z&&[^aeiou]]']) assert.equal(validate(pattern, caseSensitive), '');
    for (const pattern of ['[', '(?=a)', '(a)\\1', 'x'.repeat(4097)]) assert.notEqual(validate(pattern, caseSensitive), '');
  }
});
