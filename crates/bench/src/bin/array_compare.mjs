// Node.js baseline for array_compare.rs. Every query runs inside this process.
import fs from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';

const args = new Map();
for (let i = 2; i < process.argv.length; i += 2) args.set(process.argv[i], process.argv[i + 1]);
const input = args.get('--input');
const sourceDir = args.get('--source-dir');
const streamNdjson = args.has('--stream-ndjson');
const asyncOriginal = args.has('--async-original');
const id = args.get('--id');
const agent = args.get('--agent');
const agentsInput = args.get('--agents-input');
const seriesInput = args.get('--series-input');
const placesInput = args.get('--places-input');
const agentsSourceDir = args.get('--agents-source-dir');
const seriesSourceDir = args.get('--series-source-dir');
const placesSourceDir = args.get('--places-source-dir');
const runs = Number(args.get('--runs'));
if (!input || !id || !Number.isInteger(runs) || runs < 1) throw new Error('invalid benchmark arguments');

function emit(metric, seconds, result) {
  process.stdout.write(`${JSON.stringify({ metric, seconds, result })}\n`);
}
function median(times) {
  times.sort((a, b) => a - b);
  return times[Math.floor(times.length / 2)];
}
function measure(fn) {
  fn();
  const times = [];
  let result;
  for (let i = 0; i < runs; i++) {
    const start = performance.now();
    result = fn();
    times.push((performance.now() - start) / 1000);
  }
  return [median(times), result];
}
function load(path = input) {
  const text = fs.readFileSync(path, 'utf8');
  const lines = text.split('\n');
  if (lines.at(-1) === '') lines.pop();
  return lines.map(line => JSON.parse(line));
}
function loadNdjsonChunks(file) {
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
  files.sort();
  return files;
}
function loadOriginalFiles(directory) {
  return originalPaths(directory).map(file => ({ id: path.basename(file, '.json'), data: JSON.parse(fs.readFileSync(file, 'utf8')) }));
}
async function loadOriginalFilesAsync(directory) {
  const files = originalPaths(directory);
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
function loadCollection(ndjson, originals) {
  return originals ? loadOriginalFiles(originals) : streamNdjson ? loadNdjsonChunks(ndjson) : load(ndjson);
}
function loadChosen(ndjson, originals) {
  return asyncOriginal && originals ? loadOriginalFilesAsync(originals) : loadCollection(ndjson, originals);
}
function memory() {
  const status = fs.readFileSync('/proc/self/status', 'utf8');
  const field = name => Number(status.match(new RegExp(`^${name}:\\s+(\\d+)`, 'm'))?.[1] ?? 0);
  return { rss_kib: field('VmRSS'), peak_rss_kib: field('VmHWM'), heap_used_kib: Math.round(process.memoryUsage().heapUsed / 1024) };
}
function compareId(a, b) { return a < b ? -1 : a > b ? 1 : 0; }
function yearInRange(row) { return typeof row.data.year === 'number' && row.data.year >= 1800 && row.data.year <= 1899; }
function byYearId(a, b) { return a.data.year - b.data.year || compareId(a.id, b.id); }
function hasAgent(row) { return Array.isArray(row.data.agents) && row.data.agents.some(item => item.agent === agent); }

let start = performance.now();
let rows = await loadChosen(input, sourceDir);
emit('build', (performance.now() - start) / 1000, 'in-memory array');
emit('memory_after_build', 0, memory());

for (const [name, fn] of [
  ['year_count', () => rows.filter(yearInRange).length],
  ['year_sort100', () => rows.filter(yearInRange).sort(byYearId).slice(0, 100).map(row => [row.data.year, row.id])],
  ['year_group', () => {
    const counts = new Map();
    for (const row of rows) if (typeof row.data.year === 'number') counts.set(row.data.year, (counts.get(row.data.year) ?? 0) + 1);
    return [...counts].sort((a, b) => a[0] - b[0]);
  }],
  ['id_lookup', () => {
    const row = rows.find(row => row.id === id);
    return row ? [[row.id, row.data.year ?? null]] : [];
  }],
]) {
  const [seconds, result] = measure(fn);
  emit(name, seconds, result);
}
if (agent !== undefined) {
  for (const [name, fn] of [
    ['agent_count', () => rows.filter(hasAgent).length],
    ['agent_ids', () => rows.filter(hasAgent).map(row => row.id).sort(compareId)],
  ]) {
    const [seconds, result] = measure(fn);
    emit(name, seconds, result);
  }
}

start = performance.now();
let yearView = rows.filter(yearInRange).sort(byYearId);
emit('build_year_view', (performance.now() - start) / 1000, 'sorted array');
{
  const [seconds, result] = measure(() => yearView.slice(0, 100).map(row => [row.data.year, row.id]));
  emit('year_sort100_cached', seconds, result);
}
if (agentsInput && seriesInput && placesInput) {
  start = performance.now();
  const agents = await loadChosen(agentsInput, agentsSourceDir);
  const series = await loadChosen(seriesInput, seriesSourceDir);
  const places = await loadChosen(placesInput, placesSourceDir);
  const entryById = new Map(rows.map(row => [row.id, row]));
  const agentById = new Map(agents.map(row => [row.id, row]));
  const placeById = new Map(places.map(row => [row.id, row]));
  emit('build_related', (performance.now() - start) / 1000, 'related arrays + ID maps');
  emit('memory_after_related', 0, memory());
  const matching = row => row.data.year >= 1800 && row.data.year <= 1850
    && row.data.edit_state === 'Edited' && row.data.language.includes('ger')
    && row.data.items.length > 0 && row.data.agents.some(a => a.type === 'Herausgeber:in');
  for (const [name, fn] of [
    ['musenalm_title_page', () => rows.slice().sort((a, b) => compareId(a.data.preferred_title, b.data.preferred_title) || compareId(a.id, b.id)).slice(100, 150).map(row => [row.id, row.data.preferred_title, row.data.year])],
    ['musenalm_state_facets', () => {
      const counts = new Map();
      for (const row of rows) counts.set(row.data.edit_state, (counts.get(row.data.edit_state) ?? 0) + 1);
      return [...counts].sort((a, b) => compareId(a[0], b[0]));
    }],
    ['musenalm_multi_count', () => rows.filter(matching).length],
    ['musenalm_multi_page', () => rows.filter(matching).sort(byYearId).slice(100, 130).map(row => [row.id, row.data.year])],
    ['musenalm_title_search_count', () => rows.filter(row => row.data.preferred_title.toLowerCase().includes('almanach')).length],
    ['musenalm_title_search_page', () => rows.filter(row => row.data.preferred_title.toLowerCase().includes('almanach')).map(row => row.id).sort(compareId).slice(0, 50)],
    ['musenalm_content_types', () => {
      const counts = new Map();
      for (const row of rows) if (row.data.year >= 1800 && row.data.year <= 1899 && row.data.edit_state === 'Edited') {
        for (const content of row.data.contents) for (const type of content.musenalm_type) counts.set(type, (counts.get(type) ?? 0) + 1);
      }
      return [...counts].sort((a, b) => compareId(a[0], b[0]));
    }],
    ['musenalm_role_decades', () => {
      const counts = new Map();
      for (const row of rows) if (row.data.year >= 1800 && row.data.year <= 1899) {
        const decade = Math.floor(row.data.year / 10) * 10;
        for (const a of row.data.agents) {
          const key = JSON.stringify([decade, a.type]);
          counts.set(key, (counts.get(key) ?? 0) + 1);
        }
      }
      return [...counts].map(([key, count]) => [...JSON.parse(key), count]).sort((a, b) => a[0] - b[0] || compareId(a[1], b[1]));
    }],
    ['musenalm_top_agents', () => {
      const links = new Map();
      for (const row of rows) for (const a of row.data.agents) if (a.agent) {
        if (!links.has(a.agent)) links.set(a.agent, new Set());
        links.get(a.agent).add(row.id);
      }
      return [...links].map(([agentId, entries]) => [agentId, agentById.get(agentId)?.data.name ?? null, entries.size])
        .sort((a, b) => b[2] - a[2] || compareId(a[0], b[0])).slice(0, 20);
    }],
    ['musenalm_top_places', () => {
      const links = new Map();
      for (const row of rows) for (const placeId of row.data.places) {
        if (!links.has(placeId)) links.set(placeId, new Set());
        links.get(placeId).add(row.id);
      }
      return [...links].map(([placeId, entries]) => [placeId, placeById.get(placeId)?.data.name ?? null, entries.size])
        .sort((a, b) => b[2] - a[2] || compareId(a[0], b[0])).slice(0, 20);
    }],
    ['musenalm_series_span', () => {
      const out = [];
      for (const row of series) {
        const years = row.data.entries.map(id => entryById.get(id)?.data.year).filter(year => year >= 1800 && year <= 1899);
        if (years.length) out.push([row.id, row.data.title, years.length, Math.min(...years), Math.max(...years)]);
      }
      return out.sort((a, b) => b[2] - a[2] || compareId(a[0], b[0])).slice(0, 20);
    }],
  ]) {
    const [seconds, result] = measure(fn);
    emit(name, seconds, result);
  }
}
const midpoint = Math.floor(rows.length / 2);
start = performance.now();
rows[midpoint].data._bench_edit = 1;
emit('update_one', (performance.now() - start) / 1000, true);
start = performance.now();
for (let i = 0; i < Math.min(100, rows.length); i++) rows[i].data._bench_edit = 1;
emit('update_100', (performance.now() - start) / 1000, true);
rows = null;
yearView = null;
if (global.gc) global.gc();
start = performance.now();
rows = await loadChosen(input, sourceDir);
emit('rebuild', (performance.now() - start) / 1000, 'in-memory array');
emit('memory_peak', 0, memory());
