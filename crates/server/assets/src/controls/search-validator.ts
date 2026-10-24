import init from '../generated/search.wasm?init';

interface SearchWasm extends WebAssembly.Exports {
  memory: WebAssembly.Memory;
  search_alloc(length: number): number;
  search_free(pointer: number, length: number): void;
  search_validate(pointer: number, length: number, capacity: number, caseSensitive: number): number;
}
let engine: Promise<SearchWasm> | undefined;
export async function validatePattern(pattern: string): Promise<string> {
  const wasm = await (engine ??= init().then(instance => instance.exports as SearchWasm).catch(error => { engine = undefined; throw error; }));
  const bytes = new TextEncoder().encode(pattern);
  if (bytes.length > 4096) return 'Search pattern exceeds 4096 bytes';
  const capacity = 8192;
  const pointer = wasm.search_alloc(capacity);
  try {
    new Uint8Array(wasm.memory.buffer, pointer, bytes.length).set(bytes);
    const length = wasm.search_validate(pointer, bytes.length, capacity, 0);
    return new TextDecoder().decode(new Uint8Array(wasm.memory.buffer, pointer, length));
  } finally { wasm.search_free(pointer, capacity); }
}
