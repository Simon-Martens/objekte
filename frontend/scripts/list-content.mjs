import * as cheerio from 'cheerio';
import TurndownService from 'turndown';

export const structuredLists = new Set([
  'posthume_ausgaben',
  'sekundaerliteratur',
  'selbststaendige_drucke',
  'theaterauffuehrungen',
  'uebersetzungen',
  'unselbstaendige_drucke',
]);

const markdown = new TurndownService({ headingStyle: 'atx', bulletListMarker: '-' });
markdown.addRule('archiveMedia', {
  filter: node => node.nodeName === 'A' && (node.getAttribute('href') ?? '').startsWith('/static/'),
  replacement: (content, node) => {
    const raw = node.getAttribute('href').slice('/static/'.length);
    let path = raw;
    try { path = decodeURIComponent(raw); } catch { /* Keep the source spelling. */ }
    return `[${content}](/api/media/file?path=${encodeURIComponent(path)})`;
  },
});

function toMarkdown(html, inline = false) {
  const fragment = cheerio.load(html ?? '', null, false);
  fragment('a[href]').each((_, element) => {
    fragment(element).attr('href', (fragment(element).attr('href') ?? '').trim());
  });
  const value = markdown.turndown(fragment.html())
    .replace(/\u00a0/g, ' ')
    .replace(/<(?=[A-Za-z][\w-]*>)/g, '\\<')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n[ \t]+/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
  return inline ? value.replace(/\s+/g, ' ').trim() : value;
}

function listFields(name, body, source) {
  const $ = cheerio.load(body ?? '');
  const entry = $('.Einzug');
  const comments = $('.Kommentar').map((_, element) => toMarkdown($(element).html())).get();
  const other = $('body').children().filter((_, element) =>
    !$(element).hasClass('Einzug') && !$(element).hasClass('Kommentar') && $(element).text().trim());
  if (entry.length > 1 || other.length || (entry.length === 0 && body.trim() && name !== 'unselbstaendige_drucke')) {
    throw new Error(`${source}: unexpected list markup`);
  }

  if (name === 'unselbstaendige_drucke') {
    if (!entry.length) return { ...(comments.length ? { Kommentare: comments } : {}) };
    const parts = (entry.html() ?? '').split(/<br\s*\/?\s*>/i);
    if (parts.length !== 2) throw new Error(`${source}: expected one title/source line break`);
    const Titelzeile = toMarkdown(parts[0], true);
    const Fundstelle = toMarkdown(parts[1]);
    if (!Titelzeile || !/^In\s*:/.test(Fundstelle)) throw new Error(`${source}: expected title followed by In:`);
    return { Titelzeile, Fundstelle, ...(comments.length ? { Kommentare: comments } : {}) };
  }

  if (name === 'selbststaendige_drucke') {
    if (comments.length < 2 || !comments[0].includes('Satzspiegel') || !/^Exemplare\s*:/.test(comments.at(-1))) {
      throw new Error(`${source}: expected physical description and exemplars`);
    }
    return {
      Titelblatt: toMarkdown(entry.html()),
      Kollation: comments[0],
      Exemplare: comments.at(-1).replace(/^Exemplare\s*:\s*/, ''),
      ...(comments.length > 2 ? { Anmerkungen: comments.slice(1, -1) } : {}),
    };
  }

  return {
    Eintrag: entry.length ? toMarkdown(entry.html()) : '',
    ...(comments.length ? { Kommentare: comments } : {}),
  };
}

export function convertListRecord(name, record) {
  if (!structuredLists.has(name)) throw new Error(`Unsupported structured list: ${name}`);
  const { body, label, Name, Titel, ...rest } = record;
  const metadata = name === 'theaterauffuehrungen'
    ? { Werk: Name ?? Titel ?? '' }
    : { ...(Name !== undefined ? { Name } : {}), ...(Titel !== undefined ? { Titel } : {}) };
  return { ...rest, ...metadata, ...listFields(name, body, record.source ?? name) };
}

function jsonType(value) {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  if (typeof value === 'number') return Number.isInteger(value) ? 'integer' : 'number';
  return typeof value;
}
function inferSchema(values) {
  const types = [...new Set(values.map(jsonType))].sort();
  return { type: types.length === 1 ? types[0] : types };
}
const markdownField = title => ({ type: 'string', title, 'x-editor': 'markdown' });
const notesField = title => ({ type: 'array', title, items: markdownField('Text') });

export function listCollectionSchema(name, label, records) {
  if (!structuredLists.has(name)) throw new Error(`Unsupported structured list: ${name}`);
  const contentFields = name === 'unselbstaendige_drucke'
    ? ['Titelzeile', 'Fundstelle', 'Kommentare']
    : name === 'selbststaendige_drucke'
      ? ['Titelblatt', 'Kollation', 'Exemplare', 'Anmerkungen']
      : ['Eintrag', 'Kommentare'];
  const metadataKeys = [...new Set(records.flatMap(record => Object.keys(record.value)))].filter(key =>
    !['source', 'assets', ...contentFields].includes(key)).sort();
  const properties = {
    source: { type: 'string', title: 'Source', 'x-editor': 'hidden' },
    assets: { type: 'array', title: 'Media', 'x-editor': 'hidden', items: { type: 'string', 'x-editor': 'media' } },
  };
  for (const key of metadataKeys) {
    const values = records.filter(record => Object.hasOwn(record.value, key)).map(record => record.value[key]);
    properties[key] = { ...inferSchema(values), title: key };
    if ((name === 'theaterauffuehrungen' && key === 'Werk') || (name === 'uebersetzungen' && key === 'Sprache') || (name === 'unselbstaendige_drucke' && key === 'Werk')) {
      properties[key]['x-editor'] = 'autocomplete-select';
    }
  }
  if (name === 'unselbstaendige_drucke') {
    properties.Titelzeile = { type: 'string', title: 'Titelzeile', 'x-editor': 'inline-markdown' };
    properties.Fundstelle = markdownField('Fundstelle');
    properties.Kommentare = notesField('Kommentare');
  } else if (name === 'selbststaendige_drucke') {
    properties.Titelblatt = markdownField('Titelblatt');
    properties.Kollation = markdownField('Kollation');
    properties.Exemplare = markdownField('Exemplare');
    properties.Anmerkungen = notesField('Anmerkungen');
  } else {
    properties.Eintrag = markdownField('Eintrag');
    properties.Kommentare = notesField('Kommentare');
  }
  const display = name === 'unselbstaendige_drucke' ? 'Titelzeile' : name === 'selbststaendige_drucke' ? 'Titelblatt' : 'Eintrag';
  const displayFields = name === 'sekundaerliteratur' ? ['Jahr', 'Autor', 'Eintrag']
    : name === 'posthume_ausgaben' || name === 'selbststaendige_drucke' ? ['Jahr', display]
      : name === 'theaterauffuehrungen' ? ['Jahr', 'Werk']
        : name === 'uebersetzungen' ? ['Sprache', 'Eintrag'] : ['Werk', 'Titelzeile'];
  return {
    title: label, type: 'object',
    required: ['source', ...metadataKeys.filter(key => records.every(record => Object.hasOwn(record.value, key))),
      ...(name === 'unselbstaendige_drucke' ? [] : name === 'selbststaendige_drucke' ? ['Titelblatt', 'Kollation', 'Exemplare'] : ['Eintrag'])],
    properties, additionalProperties: false,
    'x-collection': {
      name, label, path: `data/${name}`, format: 'json', id: 'filename', display,
      displayFields, displayFallback: name === 'unselbstaendige_drucke' ? ['Fundstelle', 'Werk'] : [],
      search: [...new Set([...metadataKeys, ...contentFields])],
      fieldOrder: [...metadataKeys, ...contentFields],
    },
  };
}
