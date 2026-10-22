import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { basename, join, relative, resolve, sep } from 'node:path';
import YAML from 'yaml';
import { decodeHTML } from 'entities';
import { convertHandschriften } from './handschriften.mjs';
import { convertListRecord, listCollectionSchema, structuredLists } from './list-content.mjs';

const source = resolve(process.argv[2] ?? '../../lenz-archiv');
const destination = resolve(process.argv[3] ?? '../examples/lenz-archiv');
const dynamic = join(source, 'src/dynamic');
const staticDir = join(source, 'src/static');
if (!existsSync(dynamic) || !existsSync(staticDir)) {
  throw new Error(`Expected a complete Lenz archive checkout at ${source}`);
}
if (existsSync(destination)) {
  throw new Error(`Destination already exists: ${destination}`);
}

const labels = {
  pages: 'Pages',
  handschriften: 'Handschriften',
  orte: 'Orte',
  sammlungen: 'Sammlungen',
  posthume_ausgaben: 'Posthume Ausgaben',
  sekundaerliteratur: 'Sekundärliteratur',
  selbststaendige_drucke: 'Selbstständige Drucke',
  theaterauffuehrungen: 'Theateraufführungen',
  uebersetzungen: 'Übersetzungen',
  unselbstaendige_drucke: 'Unselbstständige Drucke',
};

function filesUnder(dir, extension) {
  return readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
    const path = join(dir, entry.name);
    return entry.isDirectory() ? filesUnder(path, extension) : entry.isFile() && path.endsWith(extension) ? [path] : [];
  }).sort();
}

function parseSource(path) {
  const text = readFileSync(path, 'utf8');
  const match = /^---\s*\r?\n([\s\S]*?)\r?\n---\s*\r?\n/.exec(text);
  if (!match) throw new Error(`Missing YAML frontmatter: ${path}`);
  const metadata = YAML.parse(match[1]);
  if (!metadata || typeof metadata !== 'object' || Array.isArray(metadata)) {
    throw new Error(`Invalid YAML frontmatter: ${path}`);
  }
  return { metadata, body: text.slice(match[0].length) };
}

function assetsIn(body) {
  const found = new Set();
  for (const match of body.matchAll(/(?:src|href)\s*=\s*["']\/static\/([^"']+)["']/gi)) {
    let path = decodeHTML(match[1]).split(/[?#]/)[0];
    try { path = decodeURIComponent(path); } catch { /* Keep the source spelling. */ }
    if (path && !path.split('/').includes('..') && existsSync(join(staticDir, path))) found.add(path);
  }
  return [...found].sort();
}

function jsonType(value) {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  if (typeof value === 'number') return Number.isInteger(value) ? 'integer' : 'number';
  return typeof value;
}

function inferSchema(values) {
  const types = [...new Set(values.map(jsonType))].sort();
  const schema = { type: types.length === 1 ? types[0] : types };
  if (types.length === 1 && types[0] === 'array') {
    const items = values.flat();
    schema.items = items.length ? inferSchema(items) : {};
  }
  if (types.length === 1 && types[0] === 'object') {
    const keys = [...new Set(values.flatMap(value => Object.keys(value)))].sort();
    schema.properties = Object.fromEntries(keys.map(key => [key, inferSchema(values.filter(value => Object.hasOwn(value, key)).map(value => value[key]))]));
    const required = keys.filter(key => values.every(value => Object.hasOwn(value, key)));
    if (required.length) schema.required = required;
    schema.additionalProperties = false;
  }
  return schema;
}

let manuscripts;
function collectionSchema(name, records) {
  if (name === 'handschriften') return manuscripts.schema;
  if (name === 'orte') return manuscripts.placeSchema;
  if (name === 'sammlungen') return manuscripts.subcollectionSchema;
  if (structuredLists.has(name)) return listCollectionSchema(name, labels[name], records);
  const metadataKeys = [...new Set(records.flatMap(record => Object.keys(record.value)))].filter(key => !['label', 'source', 'body', 'assets'].includes(key)).sort();
  const properties = {
    ...(name === 'pages' ? {} : { label: { type: 'string', minLength: 1, title: 'Label' } }),
    source: { type: 'string', title: 'Source', 'x-editor': 'hidden' },
    body: { type: 'string', title: 'HTML', 'x-editor': 'html' },
    assets: { type: 'array', title: 'Media', 'x-editor': 'hidden', items: { type: 'string', 'x-editor': 'media' } },
  };
  for (const key of metadataKeys) {
    const pageTitles = { title: 'Title', description: 'Description', eleventyNavigation: 'Navigation', layout: 'Layout', permalink: 'Permalink', showinnav: 'Show in navigation' };
    properties[key] = { ...inferSchema(records.filter(record => Object.hasOwn(record.value, key)).map(record => record.value[key])), title: name === 'pages' ? pageTitles[key] ?? key : key };
    if (name === 'pages' && key === 'eleventyNavigation') properties[key]['x-editor'] = 'json';
  }
  return {
    title: labels[name],
    type: 'object',
    required: ['source', 'body', ...(name === 'pages' ? [] : ['label']), ...metadataKeys.filter(key => records.every(record => Object.hasOwn(record.value, key)))],
    properties,
    additionalProperties: false,
    'x-collection': {
      name, label: labels[name], path: `data/${name}`, format: 'json', id: 'filename', display: name === 'pages' ? 'title' : 'label',
      search: [...(name === 'pages' ? [] : ['label']), 'body', ...metadataKeys],
    },
  };
}

function recordFor(path, name) {
  const { metadata, body } = parseSource(path);
  const sourcePath = relative(source, path).split(sep).join('/');
  const id = name === 'pages'
    ? relative(dynamic, path).replace(/\.njk$/, '').split(sep).join('--')
    : basename(path, '.html');
  if (!id || id.length > 255 || id.includes('/') || id.includes('\\')) throw new Error(`Invalid record ID: ${id}`);
  const assets = assetsIn(body);
  const value = { source: sourcePath, body, ...(assets.length ? { assets } : {}), ...metadata };
  delete value.kind;
  return { id, value: structuredLists.has(name) ? convertListRecord(name, value) : value };
}

const groups = new Map();
manuscripts = convertHandschriften(source);
groups.set('pages', filesUnder(dynamic, '.njk').filter(path => !path.includes(`${sep}lists${sep}`)).map(path => recordFor(path, 'pages')));
const lists = join(dynamic, 'lists');
for (const entry of readdirSync(lists, { withFileTypes: true }).filter(entry => entry.isDirectory()).sort((a, b) => a.name.localeCompare(b.name))) {
  const name = entry.name;
  if (!labels[name]) throw new Error(`Unknown collection: ${name}`);
  groups.set(name, name === 'handschriften'
    ? manuscripts.records
    : filesUnder(join(lists, name), '.html').map(path => recordFor(path, name)));
}
groups.set('orte', manuscripts.places);
groups.set('sammlungen', manuscripts.subcollections);

const staging = `${destination}.import-${process.pid}`;
mkdirSync(staging, { recursive: true });
try {
  mkdirSync(join(staging, '.editor'));
  for (const [name, records] of groups) {
    const dir = join(staging, 'data', name);
    mkdirSync(dir, { recursive: true });
    const ids = new Set();
    for (const record of records) {
      if (ids.has(record.id)) throw new Error(`Duplicate ID: ${name}/${record.id}`);
      ids.add(record.id);
      writeFileSync(join(dir, `${record.id}.json`), `${JSON.stringify(record.value, null, 2)}\n`);
    }
  }
  const schema = {
    $schema: 'https://json-schema.org/draft/2020-12/schema',
    title: 'Jakob Lenz Archiv',
    'x-project': { media: 'media' },
    anyOf: [...groups].map(([name, records]) => collectionSchema(name, records)),
  };
  writeFileSync(join(staging, '.editor/schema.json'), `${JSON.stringify(schema, null, 2)}\n`);
  cpSync(staticDir, join(staging, 'media'), { recursive: true });
  if (!statSync(join(staging, 'media')).isDirectory()) throw new Error('Media copy failed');
  renameSync(staging, destination);
  for (const [name, records] of groups) console.log(`${labels[name]}: ${records.length}`);
  console.log(`Media: ${destination}/media`);
} catch (error) {
  rmSync(staging, { recursive: true, force: true });
  throw error;
}
