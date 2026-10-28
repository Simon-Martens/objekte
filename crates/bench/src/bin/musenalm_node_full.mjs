// Benchmark all Musenalm data directories with Node.js. Source files are read-only.
// Usage: node --expose-gc --max-old-space-size=8192 musenalm_node_full.mjs
//   --repo examples/musenalm --mode original|original-projected|original-async|ndjson|build-cache|scan --cache /tmp/ma-cache
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { performance } from 'node:perf_hooks';

const options = new Map();
for (let i = 2; i < process.argv.length; i += 2) options.set(process.argv[i], process.argv[i + 1]);
const repo = path.resolve(options.get('--repo') ?? 'examples/musenalm');
const mode = options.get('--mode') ?? 'original';
const cache = options.get('--cache') && path.resolve(options.get('--cache'));
const runs = Number(options.get('--runs') ?? '7');
const names = ['entries', 'agents', 'series', 'places', 'items', 'pages', 'contents'];
if (!['original', 'original-projected', 'original-async', 'ndjson', 'build-cache', 'scan'].includes(mode)) throw new Error(`unknown mode: ${mode}`);
if (['ndjson', 'build-cache'].includes(mode) && !cache) throw new Error('--cache is required');
if (!Number.isInteger(runs) || runs < 1) throw new Error('--runs must be a positive integer');

function emit(metric, seconds, other = {}) {
  process.stdout.write(`${JSON.stringify({ mode, metric, seconds, ...other })}\n`);
}
function secondsSince(start) { return (performance.now() - start) / 1000; }
function memory() {
  const m = process.memoryUsage();
  const status = fs.readFileSync('/proc/self/status', 'utf8');
  const peakRssKiB = Number(status.match(/^VmHWM:\s+(\d+)/m)?.[1] ?? 0);
  return { rss_mib: m.rss / 1048576, heap_used_mib: m.heapUsed / 1048576,
    external_mib: m.external / 1048576, peak_rss_mib: peakRssKiB / 1024 };
}
function compare(a, b) { return a < b ? -1 : a > b ? 1 : 0; }
function median(times) { return times.sort((a, b) => a - b)[Math.floor(times.length / 2)]; }
function measure(fn) {
  fn();
  const times = [];
  let result;
  for (let i = 0; i < runs; i++) {
    const start = performance.now();
    result = fn();
    times.push(secondsSince(start));
  }
  const digest = crypto.createHash('sha256').update(JSON.stringify(result)).digest('hex');
  return { seconds: median(times), digest, result_size: Array.isArray(result) ? result.length : undefined,
    result: typeof result === 'number' || (Array.isArray(result) && result.length <= 3) ? result : undefined };
}
function originalPaths(directory) {
  const pending = [directory];
  const files = [];
  while (pending.length) {
    const current = pending.pop();
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const file = path.join(current, entry.name);
      if (entry.isDirectory()) pending.push(file);
      else if (entry.isFile() && entry.name.endsWith('.json')) files.push(file);
    }
  }
  return files.sort(compare);
}
function project(name, data) {
  if (name === 'contents') return {
    year: data.year, edit_state: data.edit_state, preferred_title: data.preferred_title,
    annotation: data.annotation, musenalm_type: data.musenalm_type,
    contributors: (data.contributors ?? []).map(c => ({ agents: c.agents })),
  };
  if (name === 'entries') return {
    contents: (data.contents ?? []).map(c => ({ source_id: c.source_id })),
    items: (data.items ?? []).map(c => ({ source_id: c.source_id })),
  };
  if (name === 'agents') return { name: data.name };
  return {};
}
function loadOriginal(name, projected = false) {
  return originalPaths(path.join(repo, 'data', name)).map(file => ({
    id: path.basename(file, '.json'),
    data: projected ? project(name, JSON.parse(fs.readFileSync(file, 'utf8'))) : JSON.parse(fs.readFileSync(file, 'utf8')),
  }));
}
async function loadOriginalAsync(name) {
  const files = originalPaths(path.join(repo, 'data', name));
  const rows = new Array(files.length);
  let next = 0;
  const workers = Array.from({ length: Math.min(32, files.length) }, async () => {
    for (;;) {
      const i = next++;
      if (i >= files.length) break;
      const file = files[i];
      rows[i] = { id: path.basename(file, '.json'), data: JSON.parse(await fs.promises.readFile(file, 'utf8')) };
    }
  });
  await Promise.all(workers);
  return rows;
}
function loadNdjson(name) {
  const file = path.join(cache, `${name}.ndjson`);
  const descriptor = fs.openSync(file, 'r');
  const chunk = Buffer.allocUnsafe(1024 * 1024);
  const rows = [];
  let remainder = Buffer.alloc(0);
  try {
    for (;;) {
      const bytes = fs.readSync(descriptor, chunk, 0, chunk.length, null);
      if (!bytes) break;
      const data = remainder.length ? Buffer.concat([remainder, chunk.subarray(0, bytes)]) : chunk.subarray(0, bytes);
      let start = 0;
      for (;;) {
        const end = data.indexOf(10, start);
        if (end < 0) break;
        if (end > start) rows.push(JSON.parse(data.toString('utf8', start, end)));
        start = end + 1;
      }
      remainder = Buffer.from(data.subarray(start));
    }
    if (remainder.length) rows.push(JSON.parse(remainder.toString('utf8')));
    return rows;
  } finally {
    fs.closeSync(descriptor);
  }
}
function buildCache(name) {
  const paths = originalPaths(path.join(repo, 'data', name));
  const destination = path.join(cache, `${name}.ndjson`);
  const temporary = `${destination}.tmp`;
  const descriptor = fs.openSync(temporary, 'w');
  let batch = '';
  try {
    for (const file of paths) {
      const data = JSON.parse(fs.readFileSync(file, 'utf8'));
      batch += `${JSON.stringify({ id: path.basename(file, '.json'), data })}\n`;
      if (batch.length >= 1024 * 1024) {
        fs.writeSync(descriptor, batch);
        batch = '';
      }
    }
    if (batch) fs.writeSync(descriptor, batch);
  } finally {
    fs.closeSync(descriptor);
  }
  fs.renameSync(temporary, destination);
  return { count: paths.length, bytes: fs.statSync(destination).size };
}

if (mode === 'build-cache') {
  fs.mkdirSync(cache, { recursive: true });
  const totalStart = performance.now();
  for (const name of names) {
    const start = performance.now();
    const result = buildCache(name);
    emit(`build_${name}`, secondsSince(start), result);
  }
  emit('build_total', secondsSince(totalStart), { memory: memory() });
  process.exit(0);
}
if (mode === 'scan') {
  const totalStart = performance.now();
  for (const name of names) {
    const start = performance.now();
    const paths = originalPaths(path.join(repo, 'data', name));
    let bytes = 0;
    for (const file of paths) bytes += fs.statSync(file).size;
    emit(`scan_${name}`, secondsSince(start), { count: paths.length, bytes });
  }
  emit('scan_total', secondsSince(totalStart));
  process.exit(0);
}

const collections = {};
const totalStart = performance.now();
for (const name of names) {
  const start = performance.now();
  const rows = mode === 'original' ? loadOriginal(name)
    : mode === 'original-projected' ? loadOriginal(name, true)
    : mode === 'original-async' ? await loadOriginalAsync(name) : loadNdjson(name);
  collections[name] = rows;
  emit(`load_${name}`, secondsSince(start), { count: rows.length, memory: memory() });
}
emit('load_total', secondsSince(totalStart), { count: names.reduce((n, name) => n + collections[name].length, 0), memory: memory() });
if (global.gc) {
  global.gc();
  emit('memory_after_gc', 0, { memory: memory() });
}

const indexStart = performance.now();
const byId = Object.fromEntries(names.map(name => [name, new Map(collections[name].map(row => [row.id, row]))]));
emit('build_id_maps', secondsSince(indexStart), { memory: memory() });
const contents = collections.contents;
const entries = collections.entries;
const inCentury = row => typeof row.data.year === 'number' && row.data.year >= 1800 && row.data.year <= 1899;
const editedCentury = row => inCentury(row) && row.data.edit_state === 'Edited';
const contentId = contents[Math.floor(contents.length / 2)].id;

const queries = {
  content_year_count: () => contents.filter(inCentury).length,
  content_edited_count: () => contents.filter(editedCentury).length,
  content_title_page: () => contents.filter(editedCentury)
    .sort((a, b) => compare(a.data.preferred_title, b.data.preferred_title) || compare(a.id, b.id))
    .slice(100, 150).map(row => [row.id, row.data.preferred_title, row.data.year]),
  content_type_facets: () => {
    const counts = new Map();
    for (const row of contents) if (editedCentury(row)) {
      for (const type of row.data.musenalm_type ?? []) counts.set(type, (counts.get(type) ?? 0) + 1);
    }
    return [...counts].sort((a, b) => compare(a[0], b[0]));
  },
  content_title_search_count: () => contents.filter(row =>
    (row.data.preferred_title ?? '').toLowerCase().includes('almanach')).length,
  content_text_search_count: () => contents.filter(row =>
    `${row.data.preferred_title ?? ''}\n${row.data.annotation ?? ''}`.toLowerCase().includes('almanach')).length,
  content_id_lookup: () => {
    const row = byId.contents.get(contentId);
    return row ? [[row.id, row.data.preferred_title, row.data.year]] : [];
  },
  entry_content_links: () => {
    let links = 0; let resolved = 0; let missing = 0;
    for (const entry of entries) for (const nested of entry.data.contents ?? []) {
      links++;
      if (nested.source_id && byId.contents.has(nested.source_id)) resolved++;
      else missing++;
    }
    return [links, resolved, missing];
  },
  entry_item_links: () => {
    let links = 0; let resolved = 0; let missing = 0;
    for (const entry of entries) for (const nested of entry.data.items ?? []) {
      links++;
      if (nested.source_id && byId.items.has(nested.source_id)) resolved++;
      else missing++;
    }
    return [links, resolved, missing];
  },
  top_content_contributors: () => {
    const links = new Map();
    for (const row of contents) for (const contributor of row.data.contributors ?? []) {
      const agentId = contributor.agents;
      if (!agentId) continue;
      if (!links.has(agentId)) links.set(agentId, new Set());
      links.get(agentId).add(row.id);
    }
    return [...links].map(([agentId, linked]) =>
      [agentId, byId.agents.get(agentId)?.data.name ?? null, linked.size])
      .sort((a, b) => b[2] - a[2] || compare(a[0], b[0])).slice(0, 20);
  },
};
for (const [name, fn] of Object.entries(queries)) {
  const { seconds, ...result } = measure(fn);
  emit(name, seconds, result);
}
const viewStart = performance.now();
const contentTitleView = contents.filter(editedCentury)
  .sort((a, b) => compare(a.data.preferred_title, b.data.preferred_title) || compare(a.id, b.id));
emit('build_content_title_view', secondsSince(viewStart), { count: contentTitleView.length, memory: memory() });
const { seconds: pageSeconds, ...pageResult } = measure(() => contentTitleView.slice(100, 150)
  .map(row => [row.id, row.data.preferred_title, row.data.year]));
emit('content_title_page_cached', pageSeconds, pageResult);
emit('end_memory', 0, { memory: memory() });
