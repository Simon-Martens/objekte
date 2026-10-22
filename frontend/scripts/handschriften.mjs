import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { join, relative, sep } from 'node:path';
import * as cheerio from 'cheerio';
import TurndownService from 'turndown';
import YAML from 'yaml';

const fields = {
  signatur: 'Signatur', titel: 'Titel', incipit: 'Incipit',
  umfang: 'Umfang', druck: 'Druck', anmerkung: 'Anmerkung', datei: 'Datei',
};
const markdown = new TurndownService({ headingStyle: 'atx', bulletListMarker: '-' });
markdown.addRule('lineBreak', { filter: 'br', replacement: () => '\n' });
markdown.addRule('archiveMedia', {
  filter: node => node.nodeName === 'A' && (node.getAttribute('href') ?? '').startsWith('/static/'),
  replacement: (content, node) => {
    const path = node.getAttribute('href').slice('/static/'.length);
    return `[${content}](/api/media/file?path=${encodeURIComponent(path)})`;
  },
});
const clean = text => text.replace(/\u00a0/g, ' ').replace(/\s+/g, ' ').trim();
const slug = text => clean(text).normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase()
  .replace(/ß/g, 'ss').replace(/æ/g, 'ae').replace(/œ/g, 'oe')
  .replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'ohne';
function toMarkdown($, node, multiline = false) {
  const value = markdown.turndown($(node).html() ?? '').replace(/\u00a0/g, ' ').trim();
  return multiline ? value.replace(/[ \t]+\n/g, '\n').replace(/\n[ \t]+/g, '\n').replace(/\n{3,}/g, '\n\n') : clean(value);
}
function frontmatter(path) {
  const raw = readFileSync(path, 'utf8');
  const match = /^---\s*\r?\n([\s\S]*?)\r?\n---\s*\r?\n/.exec(raw);
  if (!match) throw new Error(`Missing frontmatter: ${path}`);
  return { metadata: YAML.parse(match[1]), body: raw.slice(match[0].length) };
}
function makeId(place, signature, title, index, used) {
  const prefix = `${slug(place).slice(0, 65)}--${slug(signature || title || `entry-${index}`).slice(0, 115)}`;
  const identity = signature ? `${place}\0${signature}` : `${place}\0${title}\0${index}`;
  const digest = createHash('sha256').update(identity).digest('hex').slice(0, 10);
  let id = `${prefix}--${digest}`;
  if (used.has(id)) {
    const suffix = createHash('sha256').update(`${identity}\0${title}\0${index}`).digest('hex').slice(0, 8);
    id = `${id}--${suffix}`;
  }
  if (used.has(id)) throw new Error(`ID collision: ${id}`);
  used.add(id);
  return id;
}

export function categoryId(...parts) {
  const text = parts.join('\0');
  const prefix = parts.map((part, index) => slug(part).slice(0, index === 0 ? 65 : 100)).join('--');
  return `${prefix}--${createHash('sha256').update(text).digest('hex').slice(0, 10)}`;
}

export function ortSchema() {
  return {
    title: 'Ort', type: 'object', required: ['name', 'short'],
    properties: {
      name: { type: 'string', minLength: 1, title: 'Name' },
      short: { type: 'string', minLength: 1, title: 'Kurzform' },
      description: { type: 'string', title: 'Beschreibung', 'x-editor': 'markdown' },
      order: { type: 'integer', title: 'Reihenfolge' },
    },
    additionalProperties: false,
    'x-collection': { name: 'orte', label: 'Orte', path: 'data/orte', format: 'json', id: 'filename', display: 'short', displayFallback: ['name'], search: ['name', 'short', 'description'], showInSidebar: false },
  };
}

export function sammlungSchema() {
  return {
    title: 'Sammlung', type: 'object', required: ['Ort', 'name'],
    properties: {
      Ort: { type: 'string', title: 'Ort', 'x-editor': 'relation', 'x-relation': { collection: 'orte' } },
      name: { type: 'string', minLength: 1, title: 'Name' },
      description: { type: 'string', title: 'Beschreibung', 'x-editor': 'markdown' },
      order: { type: 'integer', title: 'Reihenfolge' },
    },
    additionalProperties: false,
    'x-collection': { name: 'sammlungen', label: 'Sammlungen', path: 'data/sammlungen', format: 'json', id: 'filename', display: 'name', search: ['name', 'description'], showInSidebar: false,
      categories: [{ field: 'Ort', label: 'Orte', itemLabel: 'Ort', collection: 'orte', display: 'short' }] },
  };
}

export function handschriftenSchema() {
  const inline = (title) => ({ type: 'string', title, 'x-editor': 'inline-markdown' });
  return {
    title: 'Handschriften', type: 'object',
    required: ['Ort'],
    properties: {
      Ort: { type: 'string', title: 'Ort', 'x-editor': 'relation', 'x-relation': { collection: 'orte', create: { nameField: 'name', copyTo: 'short' } } },
      Sammlung: { type: 'string', title: 'Sammlung', 'x-editor': 'relation', 'x-relation': { collection: 'sammlungen', scope: { field: 'Ort', source: 'Ort' }, create: { nameField: 'name', parentField: 'Ort', sourceField: 'Ort' } } },
      Signatur: inline('Signatur'), Titel: inline('Titel'), Incipit: inline('Incipit'),
      Umfang: inline('Umfang'), Druck: { type: 'string', title: 'Druck', 'x-editor': 'markdown' },
      Anmerkung: { type: 'string', title: 'Anmerkung', 'x-editor': 'markdown' },
      Datei: inline('Datei'),
      Verschollen: { type: 'boolean', title: 'Verschollen' },
      source: { type: 'string', title: 'Source', 'x-editor': 'hidden' },
      source_index: { type: 'integer', minimum: 1, title: 'Source index', 'x-editor': 'hidden' },
    },
    additionalProperties: false,
    'x-collection': {
      name: 'handschriften', label: 'Handschriften', path: 'data/handschriften',
      format: 'json', id: 'filename', display: 'Titel', displayFallback: ['Signatur', 'Anmerkung'],
      search: ['Ort', 'Sammlung', 'Signatur', 'Titel', 'Incipit', 'Umfang', 'Druck', 'Anmerkung'],
      fieldOrder: ['Ort', 'Sammlung', 'Signatur', 'Titel', 'Incipit', 'Umfang', 'Druck', 'Anmerkung', 'Datei', 'Verschollen'],
      categories: [
        { field: 'Ort', label: 'Orte', itemLabel: 'Ort', collection: 'orte', display: 'short', descriptionField: 'description', orderField: 'order' },
        { field: 'Sammlung', label: 'Sammlungen', itemLabel: 'Sammlung', collection: 'sammlungen', parentField: 'Ort', descriptionField: 'description', orderField: 'order' },
      ],
    },
  };
}

export function convertHandschriften(sourceRoot) {
  const dir = join(sourceRoot, 'src/dynamic/lists/handschriften');
  const files = readdirSync(dir).filter(name => name.endsWith('.html')).sort();
  const records = [];
  const categoryEntries = [];
  const used = new Set();
  const stats = { files: files.length, entries: 0, groupedPlaces: 0, groups: 0,
    fields: Object.fromEntries(Object.values(fields).filter(x => x !== 'Datei').map(x => [x, 0])),
    missingSignatures: 0, repeatedSignatures: 0, noteOnlyEntries: 0 };
  const signatures = new Set();
  for (const filename of files) {
    const path = join(dir, filename);
    const { metadata, body } = frontmatter(path);
    if (typeof metadata?.Ort !== 'string' || typeof metadata?.Short !== 'string') throw new Error(`Ort/Short missing: ${path}`);
    const $ = cheerio.load(body);
    let group = '', globalContext = '', groupContext = '', index = 0, groupOrder = 0, groupEntry = null;
    const placeEntry = { path: [clean(metadata.Ort)], label: clean(metadata.Short) };
    categoryEntries.push(placeEntry);
    const headings = $('.ueberschrift-gruppe').length;
    stats.groups += headings;
    if (headings) stats.groupedPlaces++;
    $('.ueberschrift-gruppe, .information, .handschrift').each((_, element) => {
      const node = $(element);
      if (node.hasClass('ueberschrift-gruppe')) {
        group = clean(node.text()); groupContext = '';
        groupEntry = { path: [placeEntry.path[0], group], label: group, order: ++groupOrder };
        categoryEntries.push(groupEntry);
        return;
      }
      if (node.hasClass('information')) {
        const note = toMarkdown($, element, true);
        if (note) {
          if (group) {
            groupContext = [groupContext, note].filter(Boolean).join('\n\n');
            groupEntry.description = groupContext;
          } else {
            globalContext = [globalContext, note].filter(Boolean).join('\n\n');
            placeEntry.description = globalContext;
          }
        }
        return;
      }
      if (!node.hasClass('handschrift')) return;
      index++;
      stats.entries++;
      const value = {
        Ort: clean(metadata.Ort), OrtKurz: clean(metadata.Short),
        source: relative(sourceRoot, path).split(sep).join('/'), source_index: index,
      };
      if (group) value.Sammlung = group;
      if (metadata.Verschollen === true) value.Verschollen = true;
      let previous = '';
      let rawSignature = '', rawTitle = '';
      node.find('.feld').each((_, field) => {
        if ($(field).closest('.handschrift')[0] !== element) return;
        const klass = ($(field).attr('class') ?? '').split(/\s+/).find(part => part && part !== 'feld');
        const key = klass ? fields[klass] : previous;
        if (!key) throw new Error(`Unrecognized field in ${filename} entry ${index}: ${klass}`);
        const target = $(field).nextAll('.wert').first();
        if (!target.length) throw new Error(`Missing value for ${key} in ${filename} entry ${index}`);
        const multiline = key === 'Anmerkung' || key === 'Druck';
        const content = toMarkdown($, target, multiline);
        if (content) value[key] = [value[key], content].filter(Boolean).join(multiline ? '\n\n' : ' ');
        if (key === 'Signatur') rawSignature = [rawSignature, clean(target.text())].filter(Boolean).join(' ');
        if (key === 'Titel') rawTitle = [rawTitle, clean(target.text())].filter(Boolean).join(' ');
        previous = key;
      });
      if (!previous) {
        const ownHtml = node.clone();
        ownHtml.find('.handschrift').remove();
        value.Anmerkung = toMarkdown($, ownHtml, true);
        stats.noteOnlyEntries++;
      }
      for (const key of Object.keys(stats.fields)) if (value[key]) stats.fields[key]++;
      if (!value.Signatur) stats.missingSignatures++;
      else {
        const signatureKey = `${value.Ort}\0${rawSignature}`;
        if (signatures.has(signatureKey)) stats.repeatedSignatures++;
        signatures.add(signatureKey);
      }
      records.push({ id: makeId(value.Ort, rawSignature, rawTitle, index, used), value });
    });
  }
  const placeIds = new Map();
  const places = categoryEntries.filter(entry => entry.path.length === 1).map(entry => {
    const id = categoryId(entry.path[0]);
    placeIds.set(entry.path[0], id);
    return { id, value: { name: entry.path[0], short: entry.label || entry.path[0], ...(entry.description ? { description: entry.description } : {}), ...(entry.order != null ? { order: entry.order } : {}) } };
  });
  const collectionIds = new Map();
  const subcollections = categoryEntries.filter(entry => entry.path.length === 2).map(entry => {
    const [place, name] = entry.path;
    const id = categoryId(place, name);
    collectionIds.set(JSON.stringify(entry.path), id);
    return { id, value: { Ort: placeIds.get(place), name: entry.label || name, ...(entry.description ? { description: entry.description } : {}), ...(entry.order != null ? { order: entry.order } : {}) } };
  });
  for (const record of records) {
    const place = record.value.Ort;
    record.value.Ort = placeIds.get(place);
    delete record.value.OrtKurz;
    if (record.value.Sammlung) {
      const id = collectionIds.get(JSON.stringify([place, record.value.Sammlung]));
      if (!id) throw new Error(`Missing Sammlung record for ${place} / ${record.value.Sammlung}`);
      record.value.Sammlung = id;
    }
    if (!record.value.Ort) throw new Error(`Missing Ort record for ${place}`);
  }
  return { records, places, subcollections, schema: handschriftenSchema(), placeSchema: ortSchema(), subcollectionSchema: sammlungSchema(), stats };
}
