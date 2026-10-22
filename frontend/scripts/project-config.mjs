import { readFileSync, writeFileSync, existsSync, renameSync, rmSync, mkdirSync } from 'node:fs';
import { join, resolve, relative, isAbsolute, dirname } from 'node:path';

const presentationKeys = ['sidebarLabel','sidebarIcon','allCollection','singularLabel','icon','list','edit','labels','ordering','sidebarGroup','position','pageSize','search','label', 'display', 'displayFields', 'displayFallback', 'showInSidebar', 'categories', 'categoryEntries', 'fieldOrder'];
const fieldKeys = ['title', 'description', 'x-editor', 'x-options', 'x-autocomplete','x-presentation','x-visible'];
const encode = key => key.replaceAll('~', '~0').replaceAll('/', '~1');
const read = path => JSON.parse(readFileSync(path, 'utf8'));
const text = value => `${JSON.stringify(value, null, 2)}\n`;
function projectPath(root, path) {
  const result = resolve(root, path), rel = relative(resolve(root), result);
  if (isAbsolute(path) || rel.startsWith('..') || isAbsolute(rel)) throw new Error(`Invalid project path: ${path}`);
  return result;
}

// Importers produce a combined in-memory definition; only split files are persisted.
export function splitConfig(input) {
  const objects = structuredClone(input);
  /** @type {{objects: string, ui: string, media: string, collections: Record<string, any>, settings: string, documents: Record<string, {schema: string, files: string[]}>}} */
  const repository = { objects: '.editor/objects.schema.json', ui: '.editor/ui.json', media: objects['x-project'].media, collections: {}, settings: 'settings.json', documents: { 'editor-settings': { schema: '.editor/settings.schema.json', files: ['settings.json'] } } };
  delete objects['x-project'];
  /** @type {{collections: Record<string, any>}} */
  const ui = { collections: {} };
  for (const branch of objects.oneOf ?? objects.anyOf) {
    const metadata = branch['x-collection'];
    const name = metadata.name;
    repository.collections[name] = { path: metadata.path, format: metadata.format, id: metadata.id };
    const presentation = Object.fromEntries(presentationKeys.filter(key => key in metadata).map(key => [key, metadata[key]]));
    if (metadata.search && !branch['x-search']) branch['x-search'] = metadata.search;
    const categories = metadata.categories ?? [];
    branch['x-categoryConstraints'] ??= categories.filter(level => level.collection).map(level => Object.fromEntries(['field', 'collection', 'parentField'].filter(key => key in level).map(key => [key, level[key]])));
    for (const level of branch['x-categoryConstraints']) level.label = level.field;
    branch['x-collection'] = name;
    // Shared definitions are expanded per collection for independently configurable controls.
    const fields = {};
    function extract(node, pointer = '') {
      if (!node || typeof node !== 'object' || Array.isArray(node)) return;
      const annotations = {};
      if (['inline-markdown', 'markdown', 'html', 'media'].includes(node['x-editor'])) node['x-content'] = node['x-editor'];
      for (const key of fieldKeys) if (key in node) { annotations[key] = node[key]; delete node[key]; }
      if (node['x-relation']?.create) { annotations['x-create'] = node['x-relation'].create; delete node['x-relation'].create; }
      if (Object.keys(annotations).length) fields[pointer] = annotations;
      for (const key of ['properties', '$defs', 'patternProperties']) for (const [name, child] of Object.entries(node[key] ?? {})) extract(child, `${pointer}/${key}/${encode(name)}`);
      for (const key of ['items', 'additionalProperties', 'contains', 'not', 'if', 'then', 'else']) extract(node[key], `${pointer}/${key}`);
      for (const key of ['oneOf', 'anyOf', 'allOf', 'prefixItems']) (node[key] ?? []).forEach((child, index) => extract(child, `${pointer}/${key}/${index}`));
    }
    if (objects.$defs) branch.$defs ??= structuredClone(objects.$defs);
    extract(branch);
    presentation.fields = fields;
    ui.collections[name] = presentation;
  }
  // Root definitions must carry content semantics but no widget configuration.
  function clean(node) {
    if (!node || typeof node !== 'object') return;
    if (['inline-markdown', 'markdown', 'html', 'media'].includes(node['x-editor'])) node['x-content'] = node['x-editor'];
    for (const key of fieldKeys) delete node[key];
    if (node['x-relation']) delete node['x-relation'].create;
    Object.values(node).forEach(clean);
  }
  clean(objects.$defs);
  return { repository, objects, ui };
}

export function readProjectDefinition(root) {
  const manifest = join(root, '.editor/repository.json');
  if (!existsSync(manifest)) return read(join(root, '.editor/schema.json'));
  const repository = read(manifest);
  if (repository.version === 2) {
    const branches = []; let media = 'media';
    for (const [name, settingsPath] of Object.entries(repository.collections)) {
      const settings = read(projectPath(root, settingsPath));
      if (settings.type === 'media') { if (name === 'media') media = settings.path; continue; }
      const branch = read(projectPath(root, settings.schema));
      const { fields = {}, ...presentation } = read(projectPath(root, settings.ui));
      for (const [pointer, annotations] of Object.entries(fields)) {
        const node = pointer === '' ? branch : pointer.slice(1).split('/').reduce((value,key) => value[key.replaceAll('~1','/').replaceAll('~0','~')],branch);
        for (const [key,value] of Object.entries(annotations)) { if (key === 'x-create') node['x-relation'].create = value; else node[key] = value; }
      }
      branch['x-collection'] = { name, path:settings.path, format:settings.format, id:settings.id, ...presentation };
      branches.push(branch);
    }
    return { 'x-project':{media}, anyOf:branches };
  }
  const objects = read(projectPath(root, repository.objects));
  const ui = read(projectPath(root, repository.ui));
  objects['x-project'] = { media: repository.media };
  for (const branch of objects.oneOf ?? objects.anyOf) {
    const name = branch['x-collection'];
    const { fields = {}, ...presentation } = ui.collections[name] ?? {};
    for (const [pointer, annotations] of Object.entries(fields)) {
      const node = pointer === '' ? branch : pointer.slice(1).split('/').reduce((value, key) => value[key.replaceAll('~1', '/').replaceAll('~0', '~')], branch);
      for (const [key, value] of Object.entries(annotations)) {
        if (key === 'x-create') node['x-relation'].create = value;
        else node[key] = value;
      }
    }
    branch['x-collection'] = { name, ...repository.collections[name], search: branch['x-search'] ?? [], ...presentation };
  }
  return objects;
}

export function writeProjectDefinition(root, definition) {
  const split = splitConfig(definition);
  const manifest = join(root, '.editor/repository.json');
  const previous = existsSync(manifest) ? read(manifest) : null;
  const repository = {version:2,ui:previous?.ui ?? '.editor/ui.json',settings:previous?.settings ?? 'settings.json',collections:{},documents:previous?.documents ?? split.repository.documents};
  const files = new Map();
  for (const branch of split.objects.anyOf ?? split.objects.oneOf) {
    const name = branch['x-collection']; delete branch['x-collection']; delete branch['x-categoryConstraints'];
    const presentation = split.ui.collections[name]; presentation.search ??= branch['x-search'] ?? []; delete branch['x-search'];
    for (const [key,field] of Object.entries(branch.properties ?? {})) {
      if (field['x-content'] === 'inline-markdown') field['x-content'] = 'markdown';
      if (['source','source_index'].includes(key)) {field['x-ownership']={kind:'imported',editable:false,onCreate:'omit'};if(branch.required)branch.required=branch.required.filter(k=>k!==key);}
    }
    for (const field of Object.values(presentation.fields ?? {})) if(field['x-editor']==='inline-markdown'){field['x-editor']='markdown';field['x-presentation']={mode:'inline'};}
    if (name==='handschriften' && branch.properties?.Sammlung && branch.properties?.Ort) {
      branch.properties.Ort['x-derived']={via:'Sammlung',field:'Ort',fallback:true};
      delete branch.properties.Sammlung['x-relation'].scope;
      branch.required=(branch.required??[]).filter(k=>k!=='Ort');branch.anyOf=[{required:['Sammlung']},{required:['Ort']}];
    }
    const folder=`.editor/collections/${name}`;
    const registration=previous?.version===2 ? previous.collections[name] : null;
    const old=registration ? read(projectPath(root,registration)) : {};
    const settings={type:'data',...split.repository.collections[name],schema:`${folder}/schema.json`,ui:`${folder}/ui.json`,operations:['read','create','update','rename','delete'],...old};
    if(settings.format==='markdown')settings.content??={body:'body',frontmatter:'*'};
    repository.collections[name]=registration??`${folder}/settings.json`;
    files.set(projectPath(root,repository.collections[name]),settings);
    files.set(projectPath(root,settings.schema),branch);
    files.set(projectPath(root,settings.ui),presentation);
  }
  if(previous?.version===2)for(const[name,path]of Object.entries(previous.collections)){const settings=read(projectPath(root,path));if(settings.type==='media')repository.collections[name]=path;}
  if(!Object.keys(repository.collections).some(name=>!split.repository.collections[name])){
    repository.collections.media='.editor/collections/media/settings.json';
    files.set(projectPath(root,repository.collections.media),{type:'media',path:split.repository.media,ui:'.editor/collections/media/ui.json',operations:['read','upload','mkdir','move','delete']});
    files.set(projectPath(root,'.editor/collections/media/ui.json'),{label:'Media',sidebarGroup:'Files'});
  }
  const globalUI=previous?.version===2 ? read(projectPath(root,previous.ui)) : {groups:[{name:'Content',label:'Content',position:0},{name:'Files',label:'Files',position:1}],defaults:{},collections:{}};
  files.set(projectPath(root,repository.ui),globalUI);files.set(manifest,repository);
  if(!previous)files.set(projectPath(root,repository.documents['editor-settings'].schema),{$schema:'https://json-schema.org/draft/2020-12/schema',title:'Editor instance settings',type:'object',additionalProperties:true});
  const settings=projectPath(root,repository.settings);if(!existsSync(settings))files.set(settings,{});
  const backups = new Map([...files.keys()].map(path => [path, existsSync(path) ? readFileSync(path) : null]));
  try {for(const[path,value]of files){mkdirSync(dirname(path),{recursive:true});const temp=`${path}.tmp-${process.pid}`;writeFileSync(temp,text(value));renameSync(temp,path);}}
  catch(error){for(const[path,previous]of backups){if(previous===null)rmSync(path,{force:true});else writeFileSync(path,previous);rmSync(`${path}.tmp-${process.pid}`,{force:true});}throw error;}
}
