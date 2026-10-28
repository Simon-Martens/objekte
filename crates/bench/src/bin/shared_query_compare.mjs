// Node worker for shared-query-compare: same NDJSON input and field-configured queries.
import fs from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';

const args = new Map();
for (let i = 2; i < process.argv.length; i += 2) args.set(process.argv[i], process.argv[i + 1]);
const input = args.get('--input');
const sourceDir = args.get('--source-dir');
const numberField = args.get('--number-field');
const textField = args.get('--text-field');
const lower = Number(args.get('--lower'));
const upper = Number(args.get('--upper'));
const needle = args.get('--needle')?.toLowerCase();
const runs = Number(args.get('--runs') ?? '7');
const projected = args.get('--projected') === 'true';
if (!(input || sourceDir) || !numberField || !textField || !needle || !Number.isInteger(runs) || runs < 1) {
  throw new Error('missing or invalid benchmark arguments');
}
const seconds = start => (performance.now() - start) / 1000;
const emit = (metric, time, result) => process.stdout.write(`${JSON.stringify({ metric, seconds: time, result })}\n`);
const memory = () => {
  const status = fs.readFileSync('/proc/self/status', 'utf8');
  const field = key => Number(status.match(new RegExp(`^${key}:\\s+(\\d+)`, 'm'))?.[1] ?? 0) / 1024;
  return { rss_mib: field('VmRSS'), peak_rss_mib: field('VmHWM'), heap_used_mib: process.memoryUsage().heapUsed / 1048576 };
};
function load() {
  if (sourceDir) {
    const pending = [sourceDir];
    const files = [];
    while (pending.length) {
      for (const entry of fs.readdirSync(pending.pop(), { withFileTypes: true })) {
        const filename = path.join(entry.parentPath, entry.name);
        if (entry.isDirectory()) pending.push(filename);
        else if (entry.isFile() && entry.name.endsWith('.json')) files.push(filename);
      }
    }
    files.sort();
    return files.map(filename => {
      const data = JSON.parse(fs.readFileSync(filename, 'utf8'));
      const id = path.basename(filename, '.json');
      return projected ? { id, number: data[numberField], text: data[textField] ?? '' } : { id, data };
    });
  }
  const fd = fs.openSync(input, 'r');
  const chunk = Buffer.allocUnsafe(1024 * 1024);
  const rows = [];
  let remainder = Buffer.alloc(0);
  try {
    for (;;) {
      const bytes = fs.readSync(fd, chunk, 0, chunk.length, null);
      if (!bytes) break;
      const data = remainder.length ? Buffer.concat([remainder, chunk.subarray(0, bytes)]) : chunk.subarray(0, bytes);
      let start = 0;
      for (;;) {
        const end = data.indexOf(10, start);
        if (end < 0) break;
        if (end > start) {
          const row = JSON.parse(data.toString('utf8', start, end));
          rows.push(projected ? { id: row.id, number: row.data[numberField], text: row.data[textField] ?? '' } : row);
        }
        start = end + 1;
      }
      remainder = Buffer.from(data.subarray(start));
    }
    if (remainder.length) {
      const row = JSON.parse(remainder.toString('utf8'));
      rows.push(projected ? { id: row.id, number: row.data[numberField], text: row.data[textField] ?? '' } : row);
    }
    return rows;
  } finally { fs.closeSync(fd); }
}
function measure(fn) {
  fn();
  const times = [];
  let result;
  for (let i = 0; i < runs; i++) {
    const start = performance.now();
    result = fn();
    times.push(seconds(start));
  }
  times.sort((a, b) => a - b);
  return [times[Math.floor(times.length / 2)], result];
}
const start = performance.now();
const rows = load();
emit('load', seconds(start), memory());
const number = row => projected ? row.number : row.data[numberField];
const numeric = row => typeof number(row) === 'number';
const inRange = row => numeric(row) && number(row) >= lower && number(row) <= upper;
const compareId = (a, b) => a < b ? -1 : a > b ? 1 : 0;
const queries = {
  range_count: () => rows.filter(inRange).length,
  range_page: () => rows.filter(inRange)
    .sort((a, b) => number(a) - number(b) || compareId(a.id, b.id))
    .slice(0, 50).map(row => [number(row), row.id]),
  number_group: () => {
    const counts = new Map();
    for (const row of rows) if (numeric(row)) counts.set(number(row), (counts.get(number(row)) ?? 0) + 1);
    return [...counts].sort((a, b) => a[0] - b[0]);
  },
  text_count: () => rows.filter(row => String(projected ? row.text : row.data[textField] ?? '').toLowerCase().includes(needle)).length,
};
for (const [name, fn] of Object.entries(queries)) {
  const [time, result] = measure(fn);
  emit(name, time, result);
}
emit('memory_end', 0, memory());
