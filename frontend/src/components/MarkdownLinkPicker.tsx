import { useEffect, useState } from 'react';
import { RiFileTextLine as FileIcon, RiImageLine as ImageIcon, RiLinksLine as LinkIcon } from '@remixicon/react';
import { api, type Collection, type MediaEntry, type RecordSummary } from '@/lib/api';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import type { MarkdownLink } from '@/lib/markdown-links';

type Kind = 'media' | 'record' | 'url';
type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  selectedText: string;
  existingLink: MarkdownLink | null;
  onInsert: (destination: string, label: string, image: boolean) => void;
};

const isImage = (path: string) => /\.(png|jpe?g|gif|webp)$/i.test(path);
const safeUrl = (value: string) => /^(https?:\/\/|mailto:|tel:|\/(?!\/)|#)/i.test(value.trim());

function linkKind(destination: string): Kind {
  if (destination.startsWith('/api/media/file?path=')) return 'media';
  if (destination.startsWith('#/collections/')) return 'record';
  return 'url';
}

export default function MarkdownLinkPicker({ open, onOpenChange, selectedText, existingLink, onInsert }: Props) {
  const [kind, setKind] = useState<Kind>('media');
  const [label, setLabel] = useState('');
  const [query, setQuery] = useState('');
  const [media, setMedia] = useState<MediaEntry[]>([]);
  const [collections, setCollections] = useState<Collection[]>([]);
  const [collection, setCollection] = useState('');
  const [records, setRecords] = useState<RecordSummary[]>([]);
  const [url, setUrl] = useState('');
  const [error, setError] = useState('');

  useEffect(() => {
    if (!open) return;
    setLabel(selectedText.replace(/\s+/g, ' ').trim());
    setError('');
    setQuery('');
    setKind(existingLink ? linkKind(existingLink.destination) : 'media');
    setUrl(existingLink?.destination ?? '');
    if (existingLink?.destination.startsWith('#/collections/')) {
      const name = existingLink.destination.slice('#/collections/'.length).split('/')[0];
      try { setCollection(decodeURIComponent(name)); } catch { setCollection(name); }
    }
  }, [open, selectedText, existingLink]);

  useEffect(() => {
    if (!open) return;
    let active = true;
    api<Collection[]>('/collections').then(items => {
      if (active) { setCollections(items); setCollection(previous => previous || items[0]?.name || ''); }
    }).catch(failure => { if (active) setError(String(failure)); });
    return () => { active = false; };
  }, [open]);

  useEffect(() => {
    if (!open || kind !== 'media') return;
    let active = true;
    const timer = setTimeout(() => {
      api<MediaEntry[]>(`/media/search?q=${encodeURIComponent(query)}&limit=40`)
        .then(items => { if (active) { setMedia(items); setError(''); } })
        .catch(failure => { if (active) { setMedia([]); setError(String(failure)); } });
    }, 120);
    return () => { active = false; clearTimeout(timer); };
  }, [open, kind, query]);

  useEffect(() => {
    if (!open || kind !== 'record' || !collection) return;
    let active = true;
    const timer = setTimeout(() => {
      api<RecordSummary[]>(`/collections/${encodeURIComponent(collection)}/records?q=${encodeURIComponent(query)}&limit=40`)
        .then(items => { if (active) { setRecords(items); setError(''); } })
        .catch(failure => { if (active) { setRecords([]); setError(String(failure)); } });
    }, 120);
    return () => { active = false; clearTimeout(timer); };
  }, [open, kind, collection, query]);

  const changeKind = (next: Kind) => { setKind(next); setQuery(''); setError(''); };
  const insert = (destination: string, fallback: string, image = false) => {
    onInsert(destination, label.trim() || fallback, image);
    onOpenChange(false);
  };

  return <Dialog open={open} onOpenChange={onOpenChange}><DialogContent className="max-w-xl">
    <DialogTitle>{existingLink ? 'Edit link' : 'Insert link'}</DialogTitle>
    <DialogDescription className="sr-only">Choose a media file, a record, or enter a URL.</DialogDescription>
    <div className="flex gap-1" role="group" aria-label="Link source">
      {(['media', 'record', 'url'] as const).map(option => <Button key={option} type="button" size="sm" variant={kind === option ? 'secondary' : 'ghost'} aria-pressed={kind === option} onClick={() => changeKind(option)}>{option === 'url' ? 'URL' : option === 'record' ? 'Record' : 'Media'}</Button>)}
    </div>
    <Input aria-label="Link text" placeholder="Link text" value={label} onChange={event => setLabel(event.target.value)} />
    {existingLink && kind === linkKind(existingLink.destination) && kind !== 'url' && <div className="flex min-w-0 items-center gap-2 text-sm"><span className="min-w-0 flex-1 truncate text-muted-foreground" title={existingLink.destination}>{existingLink.destination}</span><Button type="button" size="sm" onClick={() => insert(existingLink.destination, existingLink.label, existingLink.image)}>Update link</Button></div>}
    {kind !== 'url' && <Input aria-label={`Search ${kind}`} placeholder={kind === 'media' ? 'Search media files' : 'Search records'} value={query} onChange={event => setQuery(event.target.value)} />}
    {kind === 'record' && <Select value={collection} onValueChange={setCollection}><SelectTrigger aria-label="Collection"><SelectValue placeholder="Collection" /></SelectTrigger><SelectContent>{collections.map(item => <SelectItem key={item.name} value={item.name}>{item.label}</SelectItem>)}</SelectContent></Select>}
    {kind === 'media' && <div className="max-h-64 overflow-y-auto">{media.map(item => <div key={item.path} className="flex min-w-0 items-center gap-2 rounded-sm px-1 py-1 hover:bg-accent">
      {isImage(item.path) ? <ImageIcon size={16} className="shrink-0 text-muted-foreground" /> : <FileIcon size={16} className="shrink-0 text-muted-foreground" />}
      <span className="min-w-0 flex-1 truncate text-sm" title={item.path}>{item.path}</span>
      <Button type="button" size="sm" variant="ghost" onClick={() => insert(`/api/media/file?path=${encodeURIComponent(item.path)}`, item.name)}>Link</Button>
      {isImage(item.path) && <Button type="button" size="sm" variant="ghost" onClick={() => insert(`/api/media/file?path=${encodeURIComponent(item.path)}`, item.name, true)}>Embed</Button>}
    </div>)}{!media.length && !error && <p className="py-2 text-sm text-muted-foreground">No files found.</p>}</div>}
    {kind === 'record' && <div className="max-h-64 overflow-y-auto">{records.map(item => <button type="button" key={item.id} className="flex w-full min-w-0 items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent" onClick={() => insert(`#/collections/${encodeURIComponent(collection)}/${encodeURIComponent(item.id)}`, item.label)}><LinkIcon size={16} className="shrink-0 text-muted-foreground" /><span className="min-w-0 flex-1 truncate">{item.label}</span><span className="truncate text-xs text-muted-foreground">{item.id}</span></button>)}{!records.length && !error && <p className="py-2 text-sm text-muted-foreground">No records found.</p>}</div>}
    {kind === 'url' && <div className="flex gap-2"><Input aria-label="URL" placeholder="https://…" value={url} onChange={event => { setUrl(event.target.value); setError(''); }} onKeyDown={event => { if (event.key === 'Enter' && safeUrl(url)) insert(url.trim(), url.trim(), existingLink?.image ?? false); }} /><Button type="button" disabled={!safeUrl(url)} onClick={() => insert(url.trim(), url.trim(), existingLink?.image ?? false)}>{existingLink ? 'Update link' : 'Insert'}</Button></div>}
    {error && <p className="text-sm text-destructive">{error}</p>}
  </DialogContent></Dialog>;
}
