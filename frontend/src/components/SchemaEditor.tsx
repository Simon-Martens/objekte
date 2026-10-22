import * as React from 'react';
import { useEffect, useState } from 'react';
import { RiAddLine as Plus, RiDeleteBinLine as Trash2, RiArrowUpLine as ArrowUp, RiArrowDownLine as ArrowDown, RiArrowDownSLine as ChevronDown, RiFolderOpenLine as FolderOpen, RiFolderLine as Folder, RiFileTextLine as FileText } from '@remixicon/react';
import { RiMarkdownLine as MarkdownIcon, RiHtml5Line as HtmlIcon, RiLinksLine as RelationIcon, RiImageLine as MediaIcon, RiSearchLine as SearchIcon, RiBracesLine as ObjectIcon, RiListCheck as EnumIcon, RiGitBranchLine as UnionIcon, RiCheckboxCircleLine as BooleanIcon, RiHashtag as NumberIcon, RiSubtractLine as NullIcon, RiText as TextIcon } from '@remixicon/react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Checkbox } from '@/components/ui/checkbox';
import { Label } from '@/components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import ReactMarkdown from 'react-markdown';
import { api, type Issue, type RecordSummary, type MediaEntry } from '@/lib/api';
import { escapeInlineMarkdown, unescapeInlineMarkdown } from '@/lib/inline-markdown';

type Props = { schema: any; value: any; onChange: (value: any) => void; issues: Issue[]; readOnly?: boolean };
const pointer = (s: string) => s.replaceAll('~', '~0').replaceAll('/', '~1');
function resolve(root: any, schema: any, depth = 0): any {
  if (depth > 32) return schema;
  const ref = schema?.$ref;
  if (typeof ref !== 'string' || !ref.startsWith('#/')) return schema || {};
  const target = ref.slice(2).split('/').reduce((v: any, k: string) => v?.[k.replaceAll('~1', '/').replaceAll('~0', '~')], root);
  if (!target) return schema;
  const resolved = resolve(root, target, depth + 1);
  return { ...resolved, ...(schema.title !== undefined ? { title: schema.title } : {}), ...(schema.description !== undefined ? { description: schema.description } : {}) };
}
export function defaultValue(schema: any, depth = 0, root = schema): any {
  if (depth > 8) return null;
  schema = resolve(root, schema, depth);
  if ('default' in (schema || {})) return structuredClone(schema.default);
  if ('const' in (schema || {})) return structuredClone(schema.const);
  if (schema?.enum?.length) return structuredClone(schema.enum[0]);
  if (schema?.oneOf?.length || schema?.anyOf?.length) return defaultValue((schema.oneOf || schema.anyOf)[0], depth + 1, root);
  const type = Array.isArray(schema?.type) ? schema.type.find((t: string) => t !== 'null') : schema?.type;
  if (type === 'object' || schema?.properties) return Object.fromEntries([...new Set([...Object.keys(schema?.properties ?? {}), ...(schema?.required ?? [])])].filter(key => schema.required?.includes(key) || 'const' in (schema.properties?.[key] ?? {}) || 'default' in (schema.properties?.[key] ?? {})).map(key => [key, defaultValue(schema.properties?.[key] ?? {}, depth + 1, root)]));
  if (type === 'array' || schema?.items) return [];
  if (type === 'boolean') return false;
  if (type === 'number' || type === 'integer') return 0;
  if (type === 'null') return null;
  return '';
}
function validType(value: any, schema: any) {
  const type = schema?.type;
  if (!type) return true;
  const types = Array.isArray(type) ? type : [type];
  return types.some((t: string) => t === 'null' ? value === null : t === 'array' ? Array.isArray(value) : t === 'object' ? value !== null && typeof value === 'object' && !Array.isArray(value) : t === 'integer' ? Number.isInteger(value) : t === 'number' ? typeof value === 'number' : typeof value === t);
}
function fieldTypeIdentifier(root: any, rawSchema: any, value: any, depth = 0): string {
  const schema = resolve(root, rawSchema, depth);
  const declared = typeof schema.type === 'string' ? [schema.type] : Array.isArray(schema.type) ? schema.type : [];
  const jsonType = (item: any) => Array.isArray(item) ? 'array' : item === null ? 'null' : typeof item;
  const inferred = jsonType(value);
  const types = declared.length ? declared : schema.properties ? ['object'] : schema.items ? ['array']
    : schema.const !== undefined ? [jsonType(schema.const)]
      : schema.enum?.length ? [...new Set(schema.enum.map(jsonType))]
        : [schema.oneOf ? 'oneOf' : schema.anyOf ? 'anyOf' : inferred === 'undefined' ? 'any' : inferred];
  const type = types.join(' | ');
  if (type === 'array' && depth < 2 && schema.items && typeof schema.items === 'object') {
    const item = resolve(root, schema.items, depth + 1);
    return `array<${item['x-editor'] && item['x-editor'] !== 'hidden' ? item['x-editor'] : fieldTypeIdentifier(root, item, undefined, depth + 1)}>`;
  }
  if (schema['x-editor'] && schema['x-editor'] !== 'hidden') return `${type} · ${schema['x-editor']}`;
  if (schema.enum) return `${type} · enum`;
  if (schema.const !== undefined) return `${type} · const`;
  return type;
}
function FieldTypeIcon({ root, schema: rawSchema, value }: { root: any; schema: any; value: any }) {
  const schema = resolve(root, rawSchema);
  const types = Array.isArray(schema.type) ? schema.type : [schema.type];
  if (types.includes('array') || Array.isArray(value)) return null;
  const type = fieldTypeIdentifier(root, schema, value);
  const editor = schema['x-editor'];
  let Icon = TextIcon;
  if (editor === 'markdown' || editor === 'inline-markdown') Icon = MarkdownIcon;
  else if (editor === 'html') Icon = HtmlIcon;
  else if (editor === 'relation') Icon = RelationIcon;
  else if (editor === 'media') Icon = MediaIcon;
  else if (editor === 'autocomplete' || editor === 'autocomplete-select') Icon = SearchIcon;
  else if (editor === 'json') Icon = ObjectIcon;
  else if (schema.enum) Icon = EnumIcon;
  else if (schema.oneOf || schema.anyOf) Icon = UnionIcon;
  else if (types.includes('object') || schema.properties) Icon = ObjectIcon;
  else if (types.includes('boolean') || typeof value === 'boolean') Icon = BooleanIcon;
  else if (types.includes('number') || types.includes('integer') || typeof value === 'number') Icon = NumberIcon;
  else if ((types.length === 1 && types[0] === 'null') || (!schema.type && value === null)) Icon = NullIcon;
  return <Tooltip><TooltipTrigger asChild><button type="button" className="schema-field-type-icon" aria-label={`Field type: ${type}`}><Icon size={16} aria-hidden="true" /></button></TooltipTrigger><TooltipContent side="top">{type}</TooltipContent></Tooltip>;
}
function FieldIconAction({ label, onClick, disabled, children }: { label: string; onClick: () => void; disabled?: boolean; children: React.ReactNode }) {
  return <Tooltip><TooltipTrigger asChild><Button type="button" variant="ghost" size="icon" aria-label={label} onClick={onClick} disabled={disabled}>{children}</Button></TooltipTrigger><TooltipContent side="top">{label}</TooltipContent></Tooltip>;
}
function InlineMarkdownInput({ id, label, value, onChange }: { id: string; label?: string; value: string; onChange: (value: string) => void }) {
  const [draft, setDraft] = useState(() => unescapeInlineMarkdown(value));
  useEffect(() => { setDraft(unescapeInlineMarkdown(value)); }, [value]);
  return <Input id={id} aria-label={label} value={draft} onChange={event => { const next = event.target.value; setDraft(next); onChange(escapeInlineMarkdown(next)); }} />;
}
type CodeEditorProps = { id: string; label: string; value: string; onChange: (value: string) => void; language?: 'markdown' | 'html'; singleLine?: boolean; readOnly?: boolean };
function CodeField(props: CodeEditorProps) {
  const [Monaco, setMonaco] = useState<React.ComponentType<CodeEditorProps> | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let active = true;
    import('./CodeEditor').then(module => { if (active) setMonaco(() => module.default); }).catch(() => { if (active) setFailed(true); });
    return () => { active = false; };
  }, []);
  if (Monaco) return <Monaco {...props} />;
  if (!failed) return <div className={`code-editor px-3 py-2 text-sm text-muted-foreground ${props.singleLine ? 'h-[38px]' : 'h-[132px]'}`} role="status">Loading editor…</div>;
  return props.singleLine
    ? <InlineMarkdownInput id={props.id} label={props.label} value={props.value} onChange={props.onChange} />
    : <Textarea id={props.id} aria-label={props.label} value={props.value} onChange={event => props.onChange(event.target.value)} rows={6} readOnly={props.readOnly} />;
}
function matchesVariant(value: any, schema: any) {
  if (!validType(value, schema)) return false;
  if ('const' in schema && value !== schema.const) return false;
  if (schema.enum && !schema.enum.some((item: any) => JSON.stringify(item) === JSON.stringify(value))) return false;
  if (schema.properties && value && typeof value === 'object' && !Array.isArray(value)) {
    for (const [key, property] of Object.entries(schema.properties) as [string, any][]) {
      if (!property || typeof property !== 'object') continue;
      if ('const' in property && value[key] !== property.const) return false;
      if (property.enum && value[key] !== undefined && !property.enum.some((item: any) => JSON.stringify(item) === JSON.stringify(value[key]))) return false;
    }
  }
  return true;
}

function RelationPicker({ schema, value, onChange, readOnly, id, record }: { schema: any; value: string; onChange: (v: string) => void; readOnly?: boolean; id: string; record: any }) {
  const target = schema['x-relation']?.collection as string | undefined;
  const scope = schema['x-relation']?.scope as { field: string; source: string } | undefined;
  const create = schema['x-relation']?.create as { nameField: string; copyTo?: string; parentField?: string; sourceField?: string } | undefined;
  const parentId = scope?.source?.split('.').reduce((current: any, key: string) => current?.[key], record);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<RecordSummary | null>(null);
  const [matches, setMatches] = useState<RecordSummary[]>([]);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const listId = React.useId();
  useEffect(() => {
    if (!target || !value) { setSelected(null); if (!open) setQuery(''); return; }
    let active = true;
    api<RecordSummary>(`/relations/${encodeURIComponent(target)}/${encodeURIComponent(value)}`)
      .then(item => { if (active) { setSelected(item); if (!open) setQuery(item.label); } })
      .catch(() => { if (active) setSelected(null); });
    return () => { active = false; };
  }, [target, value]);
  useEffect(() => {
    setMatches([]);
    if (!open || !target || (scope && !parentId)) return;
    let active = true;
    const timer = setTimeout(() => {
      const params = new URLSearchParams({ q: query, limit: '50' });
      if (scope && parentId) params.append('category', String(parentId));
      api<RecordSummary[]>(`/collections/${encodeURIComponent(target)}/records?${params}`)
        .then(items => { if (active) setMatches(items); })
        .catch(() => { if (active) setMatches([]); });
    }, 150);
    return () => { active = false; clearTimeout(timer); };
  }, [target, open, query, scope?.field, parentId]);
  function choose(item: RecordSummary) { onChange(item.id); setSelected(item); setQuery(item.label); setOpen(false); setError(''); }
  async function createRecord() {
    const name = query.trim();
    if (!target || !create || !name || busy || (scope && !parentId)) return;
    setBusy(true); setError('');
    try {
      const targetSchema = await api<any>(`/collections/${encodeURIComponent(target)}/schema`);
      const draft = defaultValue(targetSchema);
      draft[create.nameField] = name;
      if (create.copyTo) draft[create.copyTo] = name;
      if (create.parentField && create.sourceField) {
        const parent = create.sourceField.split('.').reduce((current: any, key: string) => current?.[key], record);
        if (typeof parent !== 'string' || !parent) throw new Error(`Choose ${create.sourceField} first.`);
        draft[create.parentField] = parent;
      }
      const generated = await api<{ id: string }>(`/collections/${encodeURIComponent(target)}/ids/generate`, { method: 'POST', body: JSON.stringify({ value: draft }) });
      const item = await api<{ id: string }>(`/collections/${encodeURIComponent(target)}/records`, { method: 'POST', body: JSON.stringify({ id: generated.id, value: draft }) });
      choose({ id: item.id, label: name });
      window.dispatchEvent(new CustomEvent('project:collection-changed', { detail: target }));
    } catch (failure) { setError(failure instanceof Error ? failure.message : 'Could not create record.'); }
    finally { setBusy(false); }
  }
  if (readOnly) return value ? <a href={`#/collections/${encodeURIComponent(target || '')}/${encodeURIComponent(value)}`} className="text-sm text-primary hover:underline">{selected?.label || value}</a> : <p className="text-sm text-muted-foreground">—</p>;
  const mayCreate = !!create && !!query.trim() && !matches.some(item => item.label.toLocaleLowerCase() === query.trim().toLocaleLowerCase()) && (!scope || !!parentId);
  return <div className="relative" onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) { setOpen(false); setQuery(selected?.label || ''); } }}>
    <div className="relative"><Input id={id} role="combobox" aria-autocomplete="list" aria-expanded={open} aria-controls={listId} value={query} placeholder={scope && !parentId ? `Choose ${scope.source} first` : 'Select or create'} disabled={!!scope && !parentId || busy} className="pr-9" onFocus={() => setOpen(true)} onChange={event => { setQuery(event.target.value); setMatches([]); onChange(''); setSelected(null); setOpen(true); }} onKeyDown={event => { if (event.key === 'ArrowDown') { event.preventDefault(); setOpen(true); } else if (event.key === 'Escape') setOpen(false); else if ((event.key === 'Enter' || event.key === 'Tab') && open) { if (matches[0]) { event.preventDefault(); choose(matches[0]); } else if (mayCreate && event.key === 'Enter') { event.preventDefault(); void createRecord(); } } }} /><button type="button" tabIndex={-1} aria-label="Show linked records" className="absolute inset-y-0 right-0 flex w-9 items-center justify-center text-muted-foreground" onMouseDown={event => event.preventDefault()} onClick={() => { setQuery(''); setMatches([]); setOpen(true); document.getElementById(id)?.focus(); }}><ChevronDown size={16} /></button></div>
    {open && (matches.length > 0 || mayCreate) && <div id={listId} role="listbox" className="absolute z-50 mt-1 max-h-52 w-full overflow-y-auto rounded-md border bg-popover p-1 shadow-md">{matches.map(item => <button type="button" role="option" aria-selected={value === item.id} key={item.id} className="block w-full rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent" onMouseDown={event => event.preventDefault()} onClick={() => choose(item)}>{item.label}</button>)}{mayCreate && <button type="button" className="block w-full rounded-sm px-2 py-1.5 text-left text-sm text-primary hover:bg-accent" onMouseDown={event => event.preventDefault()} onClick={createRecord}>Create “{query.trim()}”</button>}</div>}
    {error && <p className="mt-1 text-xs text-destructive">{error}</p>}
  </div>;
}

function MediaPicker({ value, onChange, readOnly, id }: { value: string; onChange: (v: string) => void; readOnly?: boolean; id: string }) {
  const [open, setOpen] = useState(false);
  const [folder, setFolder] = useState('');
  const [entries, setEntries] = useState<MediaEntry[]>([]);
  useEffect(() => { if (open) api<MediaEntry[]>(`/media?path=${encodeURIComponent(folder)}`).then(setEntries).catch(() => setEntries([])); }, [open, folder]);
  return <div className="space-y-2">
    <div className="flex gap-2"><Input id={id} value={value || ''} onChange={e => onChange(e.target.value)} disabled={readOnly} placeholder="Relative media path" /><Button type="button" variant="outline" aria-expanded={open} onClick={() => setOpen(!open)}><FolderOpen size={16} /> Browse</Button></div>
    {open && <div className="rounded-md bg-card p-2 text-sm"><div className="mb-2 flex items-center justify-between"><span className="truncate text-muted-foreground">/{folder}</span><Button type="button" size="sm" variant="ghost" onClick={() => setFolder(folder.split('/').slice(0,-1).join('/'))} disabled={!folder}>Up</Button></div>{entries.length ? entries.map(entry => <button type="button" key={entry.path} className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent" onClick={() => entry.kind === 'directory' ? setFolder(entry.path) : (onChange(entry.path), setOpen(false))} disabled={readOnly && entry.kind !== 'directory'}>{entry.kind === 'directory' ? <Folder size={16} /> : <FileText size={16} />} {entry.name}</button>) : <p className="px-2 py-1 text-muted-foreground">No media in this folder.</p>}</div>}
  </div>;
}

function AutocompleteField({ id, label, mode, value, onChange, schema, collection, fieldPath, record }: { id: string; label: string; mode: 'autocomplete' | 'autocomplete-select'; value: string; onChange: (value: string) => void; schema: any; collection?: string; fieldPath: string; record: any }) {
  const listId = React.useId();
  const [open, setOpen] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const [active, setActive] = useState(0);
  const [remote, setRemote] = useState<string[]>([]);
  const spec = schema['x-autocomplete'];
  const enabled = spec !== false;
  const scopeFields: string[] = Array.isArray(spec?.scope) ? spec.scope : [];
  const scope = Object.fromEntries(scopeFields.flatMap(name => {
    const current = name.split('.').reduce((part: any, key: string) => part?.[key], record);
    return typeof current === 'string' && current ? [[name, current]] : [];
  }));
  const scopeJson = JSON.stringify(scope);
  const query = showAll ? '' : value;
  useEffect(() => {
    if (!open || !enabled || !collection || !fieldPath) { setRemote([]); return; }
    let current = true;
    const timer = setTimeout(() => {
      const params = new URLSearchParams({ field: fieldPath, q: query, scope: scopeJson, limit: '50' });
      api<string[]>(`/collections/${encodeURIComponent(collection)}/field-values?${params}`)
        .then(values => { if (current) setRemote(values); })
        .catch(() => { if (current) setRemote([]); });
    }, 150);
    return () => { current = false; clearTimeout(timer); };
  }, [open, enabled, collection, fieldPath, query, scopeJson]);
  const options = [...new Set([...(Array.isArray(schema['x-options']) ? schema['x-options'] : []), ...remote])]
    .filter((item): item is string => typeof item === 'string' && (!query || item.toLocaleLowerCase().includes(query.toLocaleLowerCase())))
    .slice(0, 50);
  const choose = (option: string) => { onChange(option); setOpen(false); setShowAll(false); setActive(0); };
  return <div className="relative" onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false); }}>
    <Input id={id} role="combobox" aria-label={label} aria-autocomplete="list" aria-expanded={open && options.length > 0} aria-controls={listId} aria-activedescendant={open && options[active] ? `${listId}-${active}` : undefined}
      className={mode === 'autocomplete-select' ? 'pr-9' : undefined} value={value ?? ''} onChange={event => { onChange(event.target.value); setShowAll(false); setActive(0); setOpen(true); }}
      onFocus={() => { if (mode === 'autocomplete' && value) setOpen(true); }}
      onKeyDown={event => {
        if (event.key === 'ArrowDown') { event.preventDefault(); setOpen(true); setActive(index => Math.min(index + (open ? 1 : 0), Math.max(options.length - 1, 0))); }
        else if (event.key === 'ArrowUp' && open) { event.preventDefault(); setActive(index => Math.max(index - 1, 0)); }
        else if (event.key === 'Escape') { setOpen(false); setShowAll(false); }
        else if ((event.key === 'Tab' || event.key === 'Enter') && open) {
          const suggestion = options[active];
          if (suggestion && suggestion !== value) { event.preventDefault(); choose(suggestion); }
          else if (event.key === 'Enter') { event.preventDefault(); setOpen(false); }
        }
      }} />
    {mode === 'autocomplete-select' && <button type="button" tabIndex={-1} aria-label={`Show ${label} values`} aria-expanded={open} className="absolute inset-y-0 right-0 flex w-9 items-center justify-center text-muted-foreground hover:text-foreground" onMouseDown={event => event.preventDefault()} onClick={() => { setShowAll(true); setActive(0); setOpen(!open || !showAll); window.document.getElementById(id)?.focus(); }}><ChevronDown size={16} /></button>}
    {open && options.length > 0 && <div id={listId} role="listbox" className="absolute z-50 mt-1 max-h-52 w-full overflow-y-auto rounded-md border bg-popover p-1 shadow-md">{options.map((option, index) => <div id={`${listId}-${index}`} key={option} role="option" aria-selected={index === active} className={`cursor-pointer rounded-sm px-2 py-1.5 text-sm ${index === active ? 'bg-accent text-accent-foreground' : 'hover:bg-accent'}`} onMouseDown={event => event.preventDefault()} onClick={() => choose(option)}>{option}</div>)}</div>}
  </div>;
}

function Field({ root, schema: rawSchema, value, onChange, path, label, issues, readOnly, depth = 0, hideLabel = false, compactItem = false, actions, record }: { root: any; schema: any; value: any; onChange: (v: any) => void; path: string; label: string; issues: Issue[]; readOnly?: boolean; depth?: number; hideLabel?: boolean; compactItem?: boolean; actions?: React.ReactNode; record: any }) {
  const fieldId = React.useId();
  const schema = resolve(root, rawSchema, depth);
  const errors = depth === 0 || !hideLabel || compactItem ? issues.filter(i => i.path === path) : [];
  const title = compactItem ? label : schema.title || label;
  const types = Array.isArray(schema.type) ? schema.type : [schema.type];
  const noSide = (types.includes('array') || Array.isArray(value)) && !actions;
  if (depth > 32) return <p className="text-sm text-destructive">Recursive field is too deep to display.</p>;
  const wrapper = (child: React.ReactNode, section = false) => compactItem
    ? <div className="schema-array-item" data-path={path}><div className="min-w-0">{child}{errors.map((error, index) => <p key={index} className="mt-1 text-xs text-destructive">{error.message}</p>)}</div><div className="schema-array-item-actions">{actions && <div className="schema-field-controls">{actions}</div>}<FieldTypeIcon root={root} schema={schema} value={value} /></div></div>
    : <div className={hideLabel ? 'min-w-0' : `schema-field-row${noSide ? ' schema-field-row--full' : ''}`} data-path={path}>
    {!hideLabel && <div className="schema-field-meta min-w-0">{section ? <p className="text-sm font-semibold">{title}</p> : <Label htmlFor={fieldId}>{title}</Label>}{schema.description && <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{schema.description}</p>}</div>}
    <div className="schema-field-value min-w-0">{child}{errors.map((error, index) => <p key={index} className="mt-1 text-xs text-destructive">{error.message}</p>)}</div>
    {!hideLabel && !noSide && <div className="schema-field-actions">{actions && <div className="schema-field-controls">{actions}</div>}<FieldTypeIcon root={root} schema={schema} value={value} /></div>}
  </div>;
  if (schema.oneOf || schema.anyOf) return wrapper(<UnionField root={root} variants={schema.oneOf || schema.anyOf} value={value} onChange={onChange} path={path} issues={issues} readOnly={readOnly} depth={depth + 1} record={record} />, true);
  if (types.includes('null') && types.length > 1) {
    const next = { ...schema, type: types.filter((type: string) => type !== 'null') };
    if (readOnly) return wrapper(value === null ? <p className="text-sm">—</p> : <Field root={root} schema={next} value={value} onChange={onChange} path={path} label={title} issues={issues} readOnly depth={depth + 1} hideLabel record={record} />, true);
    return wrapper(<div className="space-y-3"><div className="flex items-center gap-2"><Checkbox id={`${fieldId}-null`} checked={value === null} onCheckedChange={checked => onChange(checked === true ? null : defaultValue(next, 0, root))} disabled={readOnly} /><Label htmlFor={`${fieldId}-null`} className="font-normal">No value</Label></div>{value !== null && <Field root={root} schema={next} value={value} onChange={onChange} path={path} label={title} issues={issues} readOnly={readOnly} depth={depth + 1} hideLabel record={record} />}</div>, true);
  }
  if (schema.const !== undefined) return wrapper(<Input id={fieldId} value={String(schema.const)} readOnly className="bg-muted text-muted-foreground" />);
  if (schema.enum) return wrapper(<Select value={JSON.stringify(value)} onValueChange={next => onChange(JSON.parse(next))} disabled={readOnly}><SelectTrigger id={fieldId} aria-label={hideLabel ? title : undefined}><SelectValue placeholder="Choose a value" /></SelectTrigger><SelectContent>{schema.enum.map((item: any, index: number) => <SelectItem key={index} value={JSON.stringify(item)}>{String(item)}</SelectItem>)}</SelectContent></Select>);
  if (schema['x-editor'] === 'relation') return wrapper(<RelationPicker id={fieldId} schema={schema} value={value} onChange={onChange} readOnly={readOnly} record={record} />);
  if (schema['x-editor'] === 'media') return wrapper(<MediaPicker id={fieldId} value={value} onChange={onChange} readOnly={readOnly} />);
  if (schema['x-editor'] === 'json') return wrapper(<JsonField id={fieldId} value={value} onChange={onChange} readOnly={readOnly} label={title} />);
  if (types.includes('object') || schema.properties || (value && typeof value === 'object' && !Array.isArray(value))) {
    const object = (value && typeof value === 'object' && !Array.isArray(value)) ? value : {};
    const properties = schema.properties || {};
    const priority = (key: string) => {
      const order = schema['x-collection']?.fieldOrder;
      if (Array.isArray(order) && order.includes(key)) return order.indexOf(key);
      return key === 'title' || key === 'label' ? 0 : key === 'body' ? 90 : key === 'assets' ? 80 : 10;
    };
    const keys = [...new Set([...Object.keys(properties), ...(schema.required ?? []), ...Object.keys(object)])]
      .filter(key => (!('const' in (properties[key] || {})) || schema.required?.includes(key)) && properties[key]?.['x-editor'] !== 'hidden' && (path !== '' || schema['x-collection']?.id !== `frontmatter.${key}`))
      .sort((a, b) => priority(a) - priority(b) || a.localeCompare(b));
    const missing = keys.filter(key => !Object.hasOwn(object, key) && !schema.required?.includes(key));
    const set = (key: string, next: any) => {
      const updated = { ...object, [key]: next };
      if (object[key] !== next) for (const [child, childSchema] of Object.entries(properties) as [string, any][]) {
        if (childSchema?.['x-relation']?.scope?.source === key) delete updated[child];
      }
      onChange(updated);
    };
    const remove = (key: string) => { const next = { ...object }; delete next[key]; onChange(next); };
    return wrapper(<div className={`schema-fields space-y-4 ${path ? 'pl-3' : ''}`}>{keys.map(key => {
      if (!Object.hasOwn(object, key) && !schema.required?.includes(key)) return null;
      const childSchema = properties[key] || (typeof schema.additionalProperties === 'object' ? schema.additionalProperties : {});
      return <Field key={key} root={root} schema={childSchema} value={Object.hasOwn(object, key) ? object[key] : defaultValue(childSchema, 0, root)} onChange={next => set(key, next)} path={`${path}/${pointer(key)}`} label={key} issues={issues} readOnly={readOnly} depth={depth + 1} record={record} actions={!readOnly && !schema.required?.includes(key) && <FieldIconAction label={`Remove ${key}`} onClick={() => remove(key)}><Trash2 size={16} /></FieldIconAction>} />;
    })}{!readOnly && missing.length > 0 && <div className="flex flex-wrap gap-2">{missing.map(key => {
      const childSchema = properties[key] || (typeof schema.additionalProperties === 'object' ? schema.additionalProperties : {});
      return <Button key={key} type="button" size="sm" variant="outline" onClick={() => set(key, defaultValue(childSchema, 0, root))}><Plus size={14} /> Add {childSchema.title || key}</Button>;
    })}</div>}{schema.additionalProperties !== false && !readOnly && <AddProperty existing={keys} onAdd={key => set(key, defaultValue(schema.additionalProperties || {}))} />}</div>, true);
  }
  if (types.includes('array') || Array.isArray(value)) {
    const array: any[] = Array.isArray(value) ? value : [];
    const itemSchema = (index: number) => schema.prefixItems?.[index] || schema.items || {};
    const isMarkdownItem = (index: number, item: any) => {
      const resolved = resolve(root, itemSchema(index), depth + 1);
      return (resolved.type === 'string' || typeof item === 'string') && ['markdown', 'inline-markdown'].includes(resolved['x-editor']);
    };
    const update = (index: number, next: any) => onChange(array.map((item, i) => i === index ? next : item));
    const remove = (index: number) => onChange(array.filter((_, i) => i !== index));
    const move = (index: number, delta: number) => { const next = [...array]; [next[index], next[index + delta]] = [next[index + delta], next[index]]; onChange(next); };
    return wrapper(<div className="schema-fields space-y-3">{array.map((item, index) => <Field key={index} root={root} schema={itemSchema(index)} value={item} onChange={next => update(index, next)} path={`${path}/${index}`} label={`${title} ${index + 1}`} issues={issues} readOnly={readOnly} depth={depth + 1} compactItem={isMarkdownItem(index, item)} record={record} actions={!readOnly && <><FieldIconAction label={`Move ${title} ${index + 1} up`} onClick={() => move(index,-1)} disabled={index === 0}><ArrowUp size={15} /></FieldIconAction><FieldIconAction label={`Move ${title} ${index + 1} down`} onClick={() => move(index,1)} disabled={index === array.length - 1}><ArrowDown size={15} /></FieldIconAction><FieldIconAction label={`Remove ${title} ${index + 1}`} onClick={() => remove(index)}><Trash2 size={15} /></FieldIconAction></>} />)}{!readOnly && schema.items !== false && <Button type="button" variant="outline" size="sm" onClick={() => onChange([...array, defaultValue(itemSchema(array.length), 0, root)])}><Plus size={15} /> Add item</Button>}</div>, true);
  }
  if (types.includes('boolean')) return wrapper(<Checkbox id={fieldId} aria-label={hideLabel ? title : undefined} checked={value === true} onCheckedChange={checked => onChange(checked === true)} disabled={readOnly} />);
  if (types.includes('number') || types.includes('integer')) return wrapper(readOnly
    ? <p className="text-sm">{value ?? '—'}</p>
    : <Input id={fieldId} aria-label={hideLabel ? title : undefined} type="number" step={types.includes('integer') ? 1 : 'any'} value={value ?? ''} onChange={e => onChange(e.target.value === '' ? null : Number(e.target.value))} min={schema.minimum} max={schema.maximum} />);
  if (types.includes('string') || typeof value === 'string') {
    if (readOnly && schema['x-editor'] === 'html') return wrapper(<CodeField id={fieldId} label={title} value={value ?? ''} onChange={() => {}} language="html" readOnly />);
    if (readOnly) return wrapper(['markdown', 'inline-markdown'].includes(schema['x-editor'])
      ? <div className={`markdown-view text-sm ${schema['x-editor'] === 'inline-markdown' ? 'inline-markdown' : ''}`}><ReactMarkdown>{value || ''}</ReactMarkdown></div>
      : <p className="whitespace-pre-wrap text-sm">{value || '—'}</p>);
    if (schema['x-editor'] === 'markdown' || schema['x-editor'] === 'inline-markdown' || schema['x-editor'] === 'html') return wrapper(<CodeField id={fieldId} label={title} value={value ?? ''} onChange={onChange} language={schema['x-editor'] === 'html' ? 'html' : 'markdown'} singleLine={schema['x-editor'] === 'inline-markdown'} />);
    if (schema['x-editor'] === 'autocomplete' || schema['x-editor'] === 'autocomplete-select') return wrapper(<AutocompleteField id={fieldId} label={title} mode={schema['x-editor']} value={value ?? ''} onChange={onChange} schema={schema} collection={root['x-collection']?.name} fieldPath={path.split('/').slice(1).map(part => part.replaceAll('~1', '/').replaceAll('~0', '~')).join('.')} record={record} />);
    return wrapper(typeof value === 'string' && value.includes('\n')
      ? <Textarea id={fieldId} aria-label={hideLabel ? title : undefined} value={value ?? ''} onChange={e => onChange(e.target.value)} rows={12} />
      : <Input id={fieldId} aria-label={hideLabel ? title : undefined} value={value ?? ''} onChange={e => onChange(e.target.value)} minLength={schema.minLength} maxLength={schema.maxLength} />);
  }
  if (value === null) return wrapper(<p className="text-sm text-muted-foreground">null</p>, true);
  return wrapper(<JsonField id={fieldId} value={value} onChange={onChange} readOnly={readOnly} label={title} />, true);
}

function JsonField({ id, value, onChange, readOnly, label }: { id: string; value: any; onChange: (value: any) => void; readOnly?: boolean; label: string }) {
  const [draft, setDraft] = useState(JSON.stringify(value, null, 2));
  const [error, setError] = useState('');
  const committed = React.useRef('');
  useEffect(() => { if (JSON.stringify(value) === committed.current) return; setDraft(JSON.stringify(value, null, 2)); setError(''); }, [value]);
  return <><Textarea id={id} aria-label={label} className="font-mono" rows={5} value={draft} onChange={e => { setDraft(e.target.value); try { const parsed = JSON.parse(e.target.value); committed.current = JSON.stringify(parsed); onChange(parsed); setError(''); } catch { setError('Enter valid JSON.'); } }} disabled={readOnly} />{error && <p className="text-xs text-destructive">{error}</p>}</>;
}

function AddProperty({ existing, onAdd }: { existing: string[]; onAdd: (key: string) => void }) {
  const [key, setKey] = useState('');
  return <div className="flex flex-col gap-2 sm:flex-row"><Input aria-label="New property name" value={key} onChange={e => setKey(e.target.value)} placeholder="New property name" /><Button type="button" variant="outline" disabled={!key.trim() || existing.includes(key.trim())} onClick={() => { onAdd(key.trim()); setKey(''); }}>Add property</Button></div>;
}
function UnionField({ root, variants, value, onChange, path, issues, readOnly, depth, record }: { root: any; variants: any[]; value: any; onChange: (v: any) => void; path: string; issues: Issue[]; readOnly?: boolean; depth: number; record: any }) {
  const candidates = variants.map((variant, index) => matchesVariant(value, resolve(root, variant)) ? index : -1).filter(index => index >= 0);
  const [chosen, setChosen] = useState(-1);
  const selected = candidates.length === 1 ? candidates[0] : chosen;
  return <div className="space-y-3"><Select value={selected >= 0 ? String(selected) : undefined} onValueChange={selectedValue => { const index = Number(selectedValue); setChosen(index); onChange(defaultValue(variants[index], 0, root)); }} disabled={readOnly}><SelectTrigger aria-label="Schema variant"><SelectValue placeholder="Choose a variant" /></SelectTrigger><SelectContent>{variants.map((variant, index) => <SelectItem key={index} value={String(index)}>{variant.title || `Variant ${index + 1}`}</SelectItem>)}</SelectContent></Select>{selected >= 0 && <Field root={root} schema={variants[selected]} value={matchesVariant(value, resolve(root, variants[selected])) ? value : defaultValue(variants[selected], 0, root)} onChange={onChange} path={path} label="Value" issues={issues} readOnly={readOnly} depth={depth} hideLabel record={record} />}</div>;
}

export default function SchemaEditor({ schema, value, onChange, issues, readOnly }: Props) {
  return <Field root={schema} schema={schema} value={value} onChange={onChange} path="" label="Document" issues={issues} readOnly={readOnly} hideLabel record={value} />;
}
