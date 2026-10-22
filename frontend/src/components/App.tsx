import * as React from 'react';
import { useCallback, useEffect, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import { RiArrowLeftLine as ArrowLeft, RiArrowRightSLine as ChevronRight, RiArrowLeftRightLine as MoveIcon, RiArticleLine as Article, RiCheckLine as Check, RiDeleteBinLine as Trash, RiEditLine as Edit, RiFileAddLine as FilePlus2, RiFileList3Line as FileList, RiFileTextLine as FileText, RiFolderLine as Folder, RiImageLine as Image, RiLinksLine as Links, RiLogoutBoxRLine as LogOut, RiMagicLine as Magic, RiSearchLine as Search, RiSettings3Line as Settings, RiAdminLine as Shield, RiText as TextIcon, RiUserLine as UserRound, RiUpload2Line as Upload } from '@remixicon/react';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Label } from '@/components/ui/label';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { Checkbox } from '@/components/ui/checkbox';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from '@/components/ui/alert-dialog';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import SchemaEditor, { defaultValue } from '@/components/SchemaEditor';
import { api, ApiFailure, jsonBody, setCsrf, type CategoryNode, type Collection, type Document, type Issue, type MediaEntry, type RecordSummary, type Reference, type User } from '@/lib/api';

function path() {
  if (typeof window === 'undefined') return '/collections';
  return window.location.hash.slice(1) || '/collections';
}
const categoryRoute = (collection: string, values: string[]) => `/collections/${encodeURIComponent(collection)}/category/${values.map(encodeURIComponent).join('/')}`;
function useRoute() {
  const [route, setRoute] = useState(path);
  useEffect(() => { const f = () => setRoute(path()); window.addEventListener('hashchange', f); return () => window.removeEventListener('hashchange', f); }, []);
  const navigate = (target: string) => { window.location.hash = target; setRoute(target); };
  return { route, navigate };
}
function errorMessage(error: unknown) { return error instanceof Error ? error.message : 'Something went wrong'; }

export default function App() {
  const { route, navigate } = useRoute();
  const [user, setUser] = useState<User | null>(null);
  const [collections, setCollections] = useState<Collection[]>([]);
  const [trees, setTrees] = useState<Record<string, CategoryNode[]>>({});
  const [openCategories, setOpenCategories] = useState<Set<string>>(new Set());
  const [sidebarSettingsOpen, setSidebarSettingsOpen] = useState(false);
  const [sidebarSettings, setSidebarSettings] = useState<Record<string, { list?: boolean; categories?: boolean }>>(() => {
    if (typeof window === 'undefined') return {};
    try { return JSON.parse(localStorage.getItem('structured-editor.sidebar') ?? '{}'); } catch { return {}; }
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  useEffect(() => setError(''), [route]);
  const refresh = useCallback(async () => {
    try {
      const me = await api<{ user: User; csrf: string }>('/auth/me');
      setUser(me.user); setCsrf(me.csrf);
      const items = await api<Collection[]>('/collections');
      setCollections(items);
      const categoryCollections = items.filter(item => item.categories?.length);
      const categoryTrees = await Promise.all(categoryCollections.map(item => api<CategoryNode[]>(`/collections/${item.name}/categories`)));
      setTrees(Object.fromEntries(categoryCollections.map((item, index) => [item.name, categoryTrees[index]])));
    } catch { setUser(null); setCsrf(''); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { refresh(); }, [refresh]);
  const refreshCategories = useCallback(async (name: string) => {
    const affected = collections.filter(item => item.categories?.some(level => item.name === name || level.collection === name));
    const updates = await Promise.all(affected.map(async item => [item.name, await api<CategoryNode[]>(`/collections/${item.name}/categories`)] as const));
    if (updates.length) setTrees(previous => ({ ...previous, ...Object.fromEntries(updates) }));
  }, [collections]);
  useEffect(() => {
    const listener = (event: Event) => { const name = (event as CustomEvent<string>).detail; if (name) refreshCategories(name).catch(error => setError(errorMessage(error))); };
    window.addEventListener('project:collection-changed', listener);
    return () => window.removeEventListener('project:collection-changed', listener);
  }, [refreshCategories]);
  function setSidebarVisibility(name: string, key: 'list' | 'categories', visible: boolean) {
    setSidebarSettings(previous => { const next = { ...previous, [name]: { ...previous[name], [key]: visible } }; localStorage.setItem('structured-editor.sidebar', JSON.stringify(next)); return next; });
  }
  async function logout() { try { await api('/auth/logout', { method: 'POST' }); } catch {} setUser(null); setCsrf(''); navigate('/collections'); }

  if (loading) return <div className="grid min-h-screen place-items-center text-sm text-neutral-500">Loading editor…</div>;
  if (route.startsWith('/invite/')) return <AccessForm mode="invite" token={route.slice(8)} navigate={navigate} />;
  if (route.startsWith('/reset/')) return <AccessForm mode="reset" token={route.slice(7)} navigate={navigate} />;
  if (!user) return <Login onLogin={refresh} />;
  const [routePath, routeQuery] = route.split('?', 2);
  const segments = routePath.split('/').filter(Boolean).map(part => { try { return decodeURIComponent(part); } catch { return part; } });
  const requestedReturn = new URLSearchParams(routeQuery ?? '').get('from');
  const returnTo = requestedReturn?.startsWith('/collections/') ? requestedReturn : undefined;
  const collection = collections.find(c => c.name === segments[1]);
  const category = segments[2] === 'category' ? segments.slice(3) : [];
  return <TooltipProvider delayDuration={200}><div className="min-h-screen lg:flex">
    <aside className="flex flex-col bg-card lg:sticky lg:top-0 lg:h-screen lg:w-56 lg:shrink-0">
      <h1 className="px-4 py-4 text-lg font-semibold tracking-tight"><button type="button" onClick={() => navigate('/collections')}>Editor</button></h1>
      <nav aria-label="Main navigation" className="flex gap-1 overflow-x-auto px-2 pb-2 lg:block lg:flex-1 lg:space-y-1 lg:overflow-x-hidden lg:overflow-y-auto lg:py-1">
        <p className="hidden px-3 pb-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground lg:block">Collections</p>
        {collections.map(c => <React.Fragment key={c.name}>
          {(sidebarSettings[c.name]?.list ?? c.showInSidebar !== false) && <NavButton icon={c.format === 'markdown' ? <Article size={17} /> : <FileList size={17} />} active={segments[0] === 'collections' && segments[1] === c.name && !category.length} onClick={() => navigate(`/collections/${c.name}`)}>{c.label}</NavButton>}
          {segments[1] === c.name && (sidebarSettings[c.name]?.categories ?? c.showInSidebar !== false) && trees[c.name]?.map(node => <CategoryNavNode key={node.value} collection={c.name} node={node} values={[]} selected={category} open={openCategories} setOpen={setOpenCategories} navigate={navigate} />)}
        </React.Fragment>)}
        <div className="hidden h-2 lg:block" />
        <NavButton icon={<Image size={17} />} active={segments[0] === 'media'} onClick={() => navigate('/media')}>Media</NavButton>
        <NavButton icon={<UserRound size={17} />} active={segments[0] === 'account'} onClick={() => navigate('/account')}>Account</NavButton>
        <button type="button" className="flex min-w-max items-center gap-2.5 rounded-md px-3 py-1.5 text-left text-sm text-muted-foreground hover:bg-accent hover:text-foreground lg:w-full" onClick={() => setSidebarSettingsOpen(true)}><Settings size={17} /> Sidebar</button>
        {user.role === 'administrator' && <NavButton icon={<Shield size={17} />} active={segments[0] === 'admin'} onClick={() => navigate('/admin')}>Administration</NavButton>}
      </nav>
      <div className="hidden p-3 lg:block"><p className="truncate px-2 text-sm font-medium">{user.display_name}</p><p className="mb-2 truncate px-2 text-xs text-muted-foreground">{user.email}</p><Button variant="ghost" size="sm" onClick={logout}><LogOut size={15} /> Sign out</Button></div>
      <Dialog open={sidebarSettingsOpen} onOpenChange={setSidebarSettingsOpen}><DialogContent><DialogHeader><DialogTitle>Sidebar</DialogTitle><DialogDescription className="sr-only">Choose collection lists and categories to show in the sidebar.</DialogDescription></DialogHeader><div className="space-y-3">{collections.map(item => <div key={item.name} className="grid grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-4 text-sm"><span className="truncate">{item.label}</span><label className="flex items-center gap-2"><Checkbox checked={sidebarSettings[item.name]?.list ?? item.showInSidebar !== false} onCheckedChange={checked => setSidebarVisibility(item.name, 'list', checked === true)} /> List</label><label className="flex items-center gap-2"><Checkbox checked={!!item.categories?.length && (sidebarSettings[item.name]?.categories ?? item.showInSidebar !== false)} disabled={!item.categories?.length} onCheckedChange={checked => setSidebarVisibility(item.name, 'categories', checked === true)} /> Groups</label></div>)}</div><DialogFooter><Button onClick={() => setSidebarSettingsOpen(false)}>Done</Button></DialogFooter></DialogContent></Dialog>
    </aside>
    <main className="mx-auto w-full min-w-0 max-w-7xl px-4 py-5 sm:px-6 lg:px-7 lg:py-6">
      {error && <Message text={error} />}
      {segments[0] === 'collections' && !collection && <CollectionsPage collections={collections} navigate={navigate} />}
      {segments[0] === 'collections' && collection && (segments.length === 2 || segments[2] === 'category') && <CollectionPage key={route} collection={collection} category={category} tree={trees[collection.name] ?? []} editable={user.role !== 'user'} navigate={navigate} onError={setError} />}
      {segments[0] === 'collections' && collection && segments.length >= 3 && segments[2] !== 'category' && <RecordPage key={route} collection={collection} id={segments[2] === 'new' ? null : segments.slice(2).join('/')} initialCategory={segments[2] === 'new' && segments[3] !== 'prefill' ? segments.slice(3) : []} initialFields={segments[2] === 'new' && segments[3] === 'prefill' && segments[4] ? { [segments[4]]: segments[5] ?? '' } : {}} returnTo={returnTo} editable={user.role !== 'user'} navigate={navigate} onError={setError} onChanged={refreshCategories} />}
      {segments[0] === 'media' && <MediaPage editable={user.role !== 'user'} onError={setError} />}
      {segments[0] === 'account' && <AccountPage onDone={logout} onError={setError} />}
      {segments[0] === 'admin' && user.role === 'administrator' && <AdminPage onError={setError} />}
      {!['collections','media','account','admin'].includes(segments[0] || '') && <CollectionsPage collections={collections} navigate={navigate} />}
      <div className="mt-8 lg:hidden"><Button variant="ghost" onClick={logout}><LogOut size={15} /> Sign out</Button></div>
    </main>
  </div></TooltipProvider>;
}

function CategoryNavNode({ collection, node, values, selected, open, setOpen, navigate }: {
  collection: string; node: CategoryNode; values: string[]; selected: string[];
  open: Set<string>; setOpen: React.Dispatch<React.SetStateAction<Set<string>>>; navigate: (s: string) => void;
}) {
  const next = [...values, node.value];
  const key = JSON.stringify([collection, ...next]);
  const active = selected.length === next.length && next.every((value, index) => selected[index] === value);
  const expanded = open.has(key) || (selected.length > next.length && next.every((value, index) => selected[index] === value));
  return <div>
    <button type="button" title={node.label} aria-current={active ? 'page' : undefined}
      className={`flex w-full items-center gap-1.5 rounded-md py-1 pr-2 text-left text-xs hover:bg-accent hover:text-foreground ${active ? 'bg-secondary font-medium text-primary' : 'text-muted-foreground'}`}
      style={{ paddingLeft: `${18 + values.length * 13}px` }}
      onClick={() => { navigate(categoryRoute(collection, next)); if (node.children.length) setOpen(previous => new Set(previous).add(key)); }}>
      <ChevronRight size={13} className={`shrink-0 ${node.children.length ? expanded ? 'rotate-90' : '' : 'opacity-0'}`} />
      <span className="min-w-0 flex-1 truncate">{node.label}</span><span className="tabular-nums opacity-60">{node.count}</span>
    </button>
    {expanded && node.children.map(child => <CategoryNavNode key={child.value} collection={collection} node={child} values={next} selected={selected} open={open} setOpen={setOpen} navigate={navigate} />)}
  </div>;
}

function NavButton({ children, icon, active, onClick }: { children: React.ReactNode; icon: React.ReactNode; active: boolean; onClick: () => void }) {
  return <button type="button" aria-current={active ? 'page' : undefined} onClick={onClick} className={`flex w-full min-w-max items-center gap-2.5 rounded-md px-3 py-1.5 text-left text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring lg:min-w-0 ${active ? 'bg-secondary font-medium text-primary' : 'text-muted-foreground hover:bg-accent hover:text-foreground'}`}>{icon}<span className="lg:min-w-0 lg:truncate">{children}</span></button>;
}
function Message({ text, success = false }: { text: string; success?: boolean }) { return <Alert variant={success ? 'default' : 'destructive'} className={`mb-5 ${success ? 'border-primary/25 bg-secondary' : 'bg-red-50'}`}><AlertDescription>{text}</AlertDescription></Alert>; }
function Heading({ title, children }: { eyebrow?: string; title: React.ReactNode; children?: React.ReactNode }) { return <div className="mb-4 flex flex-wrap items-center justify-between gap-3"><h2 className="min-w-0 break-words text-xl font-semibold tracking-tight">{title}</h2>{children}</div>; }

function Login({ onLogin }: { onLogin: () => void }) {
  const [email, setEmail] = useState(''); const [password, setPassword] = useState(''); const [error, setError] = useState(''); const [busy, setBusy] = useState(false);
  async function submit(e: React.SyntheticEvent<HTMLFormElement>) { e.preventDefault(); setBusy(true); setError(''); try { await api('/auth/login', { method: 'POST', body: jsonBody({ email, password }) }); onLogin(); } catch (e) { setError(errorMessage(e)); } finally { setBusy(false); } }
  return <div className="grid min-h-screen place-items-center px-4 py-10"><Card className="w-full max-w-md"><CardHeader><CardTitle className="text-2xl">Sign in</CardTitle></CardHeader><CardContent><form onSubmit={submit} className="space-y-4">{error && <Message text={error} />}<div className="space-y-2"><Label htmlFor="login-email">Email address</Label><Input id="login-email" type="email" autoComplete="email" value={email} onChange={e => setEmail(e.target.value)} required /></div><div className="space-y-2"><Label htmlFor="login-password">Password</Label><Input id="login-password" type="password" autoComplete="current-password" value={password} onChange={e => setPassword(e.target.value)} required /></div><Button className="w-full" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</Button></form></CardContent></Card></div>;
}
function AccessForm({ mode, token, navigate }: { mode: 'invite' | 'reset'; token: string; navigate: (s: string) => void }) {
  const [email, setEmail] = useState(''); const [displayName, setDisplayName] = useState(''); const [password, setPassword] = useState(''); const [error, setError] = useState(''); const [done, setDone] = useState(false);
  async function submit(e: React.SyntheticEvent<HTMLFormElement>) { e.preventDefault(); setError(''); try { await api(mode === 'invite' ? '/auth/redeem-invitation' : '/auth/reset-password', { method: 'POST', body: jsonBody(mode === 'invite' ? { token, email, display_name: displayName, password } : { token, password }) }); setDone(true); } catch (e) { setError(errorMessage(e)); } }
  return <div className="grid min-h-screen place-items-center px-4 py-10"><Card className="w-full max-w-md"><CardHeader><CardTitle className="text-2xl">{mode === 'invite' ? 'Accept invitation' : 'Reset password'}</CardTitle></CardHeader><CardContent>{done ? <div className="space-y-4"><Message text={mode === 'invite' ? 'Your account is ready.' : 'Your password has been updated.'} success /><Button onClick={() => navigate('/collections')}>Sign in</Button></div> : <form onSubmit={submit} className="space-y-4">{error && <Message text={error} />}{mode === 'invite' && <><div className="space-y-2"><Label htmlFor="access-email">Email address</Label><Input id="access-email" type="email" autoComplete="email" value={email} onChange={e => setEmail(e.target.value)} required /></div><div className="space-y-2"><Label htmlFor="access-name">Display name</Label><Input id="access-name" value={displayName} onChange={e => setDisplayName(e.target.value)} required /></div></>}<div className="space-y-2"><Label htmlFor="access-password">New password</Label><Input id="access-password" type="password" autoComplete="new-password" minLength={12} value={password} onChange={e => setPassword(e.target.value)} required placeholder="At least 12 characters" /></div><Button className="w-full">Continue</Button></form>}</CardContent></Card></div>;
}

function CollectionsPage({ collections, navigate }: { collections: Collection[]; navigate: (s: string) => void }) {
  return <><Heading title="Collections" /><div className="grid gap-1 md:grid-cols-2">{collections.map(c => <button type="button" key={c.name} className="flex w-full items-center gap-3 rounded-md px-3 py-2 text-left text-sm font-medium hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => navigate(`/collections/${c.name}`)}>{c.format === 'markdown' ? <Article size={17} /> : <FileList size={17} />}<span className="min-w-0 flex-1 truncate">{c.label}</span><ChevronRight size={16} className="text-muted-foreground" /></button>)}</div></>;
}
function CollectionPage({ collection, category, tree, editable, navigate, onError }: { collection: Collection; category: string[]; tree: CategoryNode[]; editable: boolean; navigate: (s: string) => void; onError: (s: string) => void }) {
  const [query, setQuery] = useState(''); const [offset, setOffset] = useState(0); const [records, setRecords] = useState<RecordSummary[]>([]); const [hasNext, setHasNext] = useState(false);
  useEffect(() => {
    let active = true;
    const params = new URLSearchParams({ q: query, offset: String(offset), limit: '26' });
    category.forEach(value => params.append('category', value));
    const timer = setTimeout(() => api<RecordSummary[]>(`/collections/${collection.name}/records?${params}`).then(items => { if (active) { setRecords(items.slice(0, 25)); setHasNext(items.length > 25); } }).catch(error => { if (active) onError(errorMessage(error)); }), 150);
    return () => { active = false; clearTimeout(timer); };
  }, [collection.name, query, offset, JSON.stringify(category)]);
  let nodes = tree;
  let selectedNode: CategoryNode | undefined;
  const labels = category.map(value => { selectedNode = nodes.find(item => item.value === value); nodes = selectedNode?.children ?? []; return selectedNode?.label ?? value; });
  const childNodes = category.length ? selectedNode?.children ?? [] : tree;
  const level = collection.categories[category.length - 1];
  const selectedId = selectedNode?.value;
  const nextLevel = collection.categories[category.length];
  const showRecords = category.length > 0 || childNodes.length === 0;
  const createLinked = nextLevel?.collection ? `/collections/${nextLevel.collection}/new${nextLevel.parentField && category.length ? `/prefill/${encodeURIComponent(nextLevel.parentField)}/${encodeURIComponent(category.at(-1)!)}` : ''}` : null;
  return <>
    <Heading title={labels.at(-1) ?? collection.label}>{selectedId && level?.collection && editable && <Button variant="outline" size="sm" onClick={() => navigate(`/collections/${level.collection}/${encodeURIComponent(selectedId)}?from=${encodeURIComponent(categoryRoute(collection.name, category))}`)}><Edit size={15} /> Edit {level.itemLabel ?? 'category'}</Button>}</Heading>
    {category.length > 0 && <div className="mb-3 flex flex-wrap gap-1 text-xs text-muted-foreground"><button type="button" className="hover:text-foreground" onClick={() => navigate(`/collections/${collection.name}`)}>{collection.label}</button>{labels.map((label, index) => <React.Fragment key={index}><span>/</span><button type="button" className="hover:text-foreground" onClick={() => navigate(categoryRoute(collection.name, category.slice(0, index + 1)))}>{label}</button></React.Fragment>)}</div>}
    {selectedNode?.description && <div className="markdown-view mb-4 text-sm"><ReactMarkdown>{selectedNode.description}</ReactMarkdown></div>}
    {childNodes.length > 0 && <div className="mb-4 space-y-0.5">{childNodes.map(node => <button type="button" key={node.value} className="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm hover:bg-accent" onClick={() => navigate(categoryRoute(collection.name, [...category, node.value]))}><Folder size={16} className="shrink-0 text-muted-foreground" /><span className="min-w-0 flex-1 truncate">{node.label}</span><span className="text-xs tabular-nums text-muted-foreground">{node.count}</span><ChevronRight size={15} className="text-muted-foreground" /></button>)}</div>}
    <div className="mb-4 flex flex-col gap-3 sm:flex-row">{showRecords && <div className="relative min-w-0 flex-1"><Search size={16} className="pointer-events-none absolute left-3 top-2.5 text-muted-foreground" /><Input aria-label="Search records" value={query} onChange={event => { setQuery(event.target.value); setOffset(0); }} placeholder="Search records" className="pl-9" /></div>}{editable && <div className="flex gap-2">{createLinked && <Button variant="outline" onClick={() => navigate(`${createLinked}?from=${encodeURIComponent(category.length ? categoryRoute(collection.name, category) : `/collections/${collection.name}`)}`)}><FilePlus2 size={16} /> New {nextLevel.itemLabel ?? 'category'}</Button>}<Button onClick={() => navigate(`/collections/${collection.name}/new${category.length ? `/${category.map(encodeURIComponent).join('/')}` : ''}`)}><FilePlus2 size={16} /> New record</Button></div>}</div>
    {showRecords && <Card><div className="space-y-0.5">{records.map(record => <button type="button" key={record.id} className="flex w-full min-w-0 items-center gap-3 rounded-md px-3 py-2.5 text-left hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring" onClick={() => navigate(`/collections/${collection.name}/${encodeURIComponent(record.id)}`)}><span className="min-w-0 flex-1 truncate font-medium">{record.label}</span><span className="hidden max-w-[35%] truncate text-xs text-muted-foreground sm:block">{record.id}</span><ChevronRight size={16} className="shrink-0 text-muted-foreground" /></button>)}{!records.length && <p className="px-3 py-6 text-sm text-muted-foreground">No records found.</p>}</div></Card>}
    {showRecords && (offset > 0 || hasNext) && <div className="mt-4 flex items-center justify-between gap-3"><Button variant="outline" size="sm" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - 25))}>Previous</Button><span className="text-sm text-muted-foreground">{records.length ? `${offset + 1}–${offset + records.length}` : '0 records'}</span><Button variant="outline" size="sm" disabled={!hasNext} onClick={() => setOffset(offset + 25)}>Next</Button></div>}
  </>;
}

function RecordPage({ collection, id, initialCategory, initialFields, returnTo, editable, navigate, onError, onChanged }: { collection: Collection; id: string | null; initialCategory: string[]; initialFields: Record<string, string>; returnTo?: string; editable: boolean; navigate: (s: string) => void; onError: (s: string) => void; onChanged: (name: string) => Promise<void> }) {
  const [schema, setSchema] = useState<any>(null); const [value, setValue] = useState<any>({}); const [revision, setRevision] = useState(''); const [newId, setNewId] = useState(id ?? '');
  const [idIssues, setIdIssues] = useState<Issue[]>([]); const [idChecking, setIdChecking] = useState(false); const [idGenerating, setIdGenerating] = useState(false);
  const [issues, setIssues] = useState<Issue[]>([]); const [raw, setRaw] = useState(false); const [rawText, setRawText] = useState(''); const [success, setSuccess] = useState(''); const [busy, setBusy] = useState(false);
  const [references, setReferences] = useState<Reference[]>([]);
  const [preview, setPreview] = useState(Boolean(id));
  useEffect(() => { Promise.all([api<any>(`/collections/${collection.name}/schema`), id ? api<Document>(`/collections/${collection.name}/records/${encodeURIComponent(id)}`) : Promise.resolve(null), id ? api<Reference[]>(`/relations/${collection.name}/${encodeURIComponent(id)}/references`) : Promise.resolve([])]).then(([s,d,refs]) => { setSchema(s); setReferences(refs); setPreview(Boolean(id)); const initial = d?.value ?? defaultValue(s); if (!d) { initialCategory.forEach((part, index) => { const path = collection.categories[index]?.field.split('.') ?? []; let target = initial; path.forEach((key, i) => { if (i === path.length - 1) target[key] = part; else target = target[key] ??= {}; }); }); Object.assign(initial, initialFields); } setValue(initial); setNewId(d?.id ?? ''); setRevision(d?.revision ?? ''); setRawText(JSON.stringify(initial, null, 2)); }).catch(e => onError(errorMessage(e))); }, [collection.name, id]);
  useEffect(() => {
    if (!newId || newId === id) { setIdIssues([]); setIdChecking(false); return; }
    let active = true;
    setIdChecking(true);
    const timer = setTimeout(() => api<Issue[]>(`/collections/${collection.name}/ids/check?id=${encodeURIComponent(newId)}`)
      .then(found => { if (active) setIdIssues(found); })
      .catch(error => { if (active) setIdIssues([{ path: '/id', message: errorMessage(error) }]); })
      .finally(() => { if (active) setIdChecking(false); }), 250);
    return () => { active = false; clearTimeout(timer); };
  }, [collection.name, id, newId]);
  function toggleRaw() { if (!raw) setRawText(JSON.stringify(value, null, 2)); else { try { setValue(JSON.parse(rawText)); } catch { onError('Raw JSON is invalid.'); return; } } setRaw(!raw); }
  function togglePreview() {
    if (!preview && raw) {
      try { setValue(JSON.parse(rawText)); }
      catch { onError('Raw JSON is invalid.'); return; }
    }
    setPreview(!preview); setRaw(false);
  }
  async function generateId() {
    setIdGenerating(true); setIdIssues([]);
    try {
      const draft = raw ? JSON.parse(rawText) : value;
      const result = await api<{ id: string }>(`/collections/${collection.name}/ids/generate`, { method: 'POST', body: jsonBody({ value: draft }) });
      setNewId(result.id);
    } catch (error) { onError(errorMessage(error)); }
    finally { setIdGenerating(false); }
  }
  async function save() {
    setBusy(true); setIssues([]); setSuccess(''); onError('');
    try {
      let next = raw ? JSON.parse(rawText) : value;
      const strategy = schema?.['x-collection']?.id;
      if (typeof strategy === 'string' && strategy.startsWith('frontmatter.')) next = { ...next, [strategy.slice('frontmatter.'.length)]: newId };
      else if (schema?.properties?.id && (!id ? !next.id : next.id === id)) next = { ...next, id: newId };
      const doc = id ? await api<Document>(`/collections/${collection.name}/records/${encodeURIComponent(id)}`, { method:'PUT', body: jsonBody({ id: newId, value: next, revision }) }) : await api<Document>(`/collections/${collection.name}/records`, { method:'POST', body: jsonBody({ id: newId, value: next }) });
      setValue(doc.value); setRevision(doc.revision); setRawText(JSON.stringify(doc.value,null,2)); setSuccess('Saved to disk.'); setPreview(true);
      await onChanged(collection.name);
      if (id !== doc.id) navigate(`/collections/${collection.name}/${encodeURIComponent(doc.id)}${returnTo ? `?from=${encodeURIComponent(returnTo)}` : ''}`);
    } catch (e) { if (e instanceof ApiFailure) { setIssues(e.issues); const invalidId = e.issues.filter(issue => issue.path === '/id'); if (invalidId.length) setIdIssues(invalidId); } onError(errorMessage(e)); } finally { setBusy(false); }
  }
  async function remove() { if (!id) return; try { await api(`/collections/${collection.name}/records/${encodeURIComponent(id)}`, { method:'DELETE', body:jsonBody({ revision }) }); await onChanged(collection.name); navigate(`/collections/${collection.name}`); } catch(e) { onError(errorMessage(e)); } }
  const rawError = raw ? (() => { try { JSON.parse(rawText); return ''; } catch (error) { return errorMessage(error); } })() : '';
  const categoryValues: string[] = [];
  for (const level of collection.categories ?? []) {
    const fieldValue = level.field.split('.').reduce((current: any, part) => current?.[part], value);
    if (typeof fieldValue !== 'string' || !fieldValue) break;
    categoryValues.push(fieldValue);
  }
  const backRoute = returnTo ?? (categoryValues.length ? categoryRoute(collection.name, categoryValues) : `/collections/${collection.name}`);
  const compositeTitle = collection.displayFields?.map(field => field.split('.').reduce((current: any, part) => current?.[part], value))
    .filter(part => (typeof part === 'string' && part.trim()) || typeof part === 'number').join(' · ');
  const recordTitle = compositeTitle || [collection.display, ...(collection.displayFallback ?? [])]
    .map(field => field.split('.').reduce((current: any, part) => current?.[part], value))
    .find(text => typeof text === 'string' && text.trim()) || id;
  return <>
    <Heading title={id ? <ReactMarkdown components={{ p: ({ children }) => <span>{children}</span>, a: ({ children }) => <span>{children}</span> }}>{String(recordTitle)}</ReactMarkdown> : 'New record'}><Button variant="ghost" onClick={() => navigate(backRoute)} aria-label="Back" title="Back"><ArrowLeft size={16} /> Back</Button></Heading>
    {success && <Message text={success} success />}
    <Card><CardContent className="space-y-5 pt-4">
      <div className="schema-fields"><div className="schema-field-row"><div className="schema-field-meta">{preview || !editable ? <span className="text-sm font-medium">Record ID</span> : <Label htmlFor="record-id">Record ID</Label>}</div><div className="schema-field-value">{preview || !editable ? <p className="break-all text-sm">{newId || '—'}</p> : <><Input id="record-id" value={newId} onChange={e => setNewId(e.target.value)} placeholder="Enter an ID" aria-invalid={idIssues.length > 0} aria-describedby={idIssues.length ? 'record-id-error' : undefined} />{idIssues.length > 0 && <p id="record-id-error" className="mt-1 text-xs text-destructive">{idIssues[0].message}</p>}</>}</div><div className="schema-field-actions">{editable && !preview && <div className="schema-field-controls"><Tooltip><TooltipTrigger asChild><Button type="button" variant="ghost" size="icon" aria-label="Generate unique ID" onClick={generateId} disabled={idGenerating || !!rawError}><Magic size={16} /></Button></TooltipTrigger><TooltipContent side="top">Generate unique ID</TooltipContent></Tooltip></div>}<Tooltip><TooltipTrigger asChild><button type="button" className="schema-field-type-icon" aria-label="Field type: string"><TextIcon size={16} aria-hidden="true" /></button></TooltipTrigger><TooltipContent side="top">string</TooltipContent></Tooltip></div></div></div>
      {schema ? raw && !preview ? <div className="space-y-2"><Label htmlFor="raw-json">Raw JSON</Label><Textarea id="raw-json" className="min-h-[28rem] font-mono" spellCheck={false} value={rawText} onChange={e => setRawText(e.target.value)} disabled={!editable} />{rawError && <p className="text-sm text-destructive">{rawError}</p>}</div> : <SchemaEditor schema={schema} value={value} onChange={setValue} issues={issues} readOnly={!editable || preview} /> : <p className="text-sm text-muted-foreground">Loading schema…</p>}
      <div className="flex flex-wrap items-center justify-between gap-3 pt-2"><div className="flex gap-2">{id && editable && <Button variant="outline" onClick={togglePreview}>{preview ? 'Edit' : 'Preview'}</Button>}{editable && !preview && <Button variant="outline" onClick={toggleRaw}>{raw ? 'Form view' : 'Raw JSON'}</Button>}{id && editable && !preview && <AlertDialog><AlertDialogTrigger asChild><Button variant="outline" className="text-destructive hover:text-destructive">Delete</Button></AlertDialogTrigger><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Delete {id}?</AlertDialogTitle><AlertDialogDescription>This removes the record from the project. Records referenced by other entries cannot be deleted.</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>Cancel</AlertDialogCancel><AlertDialogAction className="bg-destructive text-destructive-foreground hover:bg-destructive/90" onClick={remove}>Delete record</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>}</div>{editable && !preview && <Button onClick={save} disabled={busy || !schema || !newId || idChecking || idIssues.length > 0 || !!rawError}>{busy ? 'Saving…' : 'Save record'}</Button>}</div>
    </CardContent></Card>
    {id && references.length > 0 && <Card className="mt-5"><CardHeader><CardTitle className="text-base">Referenced by</CardTitle></CardHeader><CardContent className="space-y-2">{references.map((ref, index) => <button type="button" key={`${ref.collection}/${ref.record_id}/${ref.path}/${index}`} className="flex w-full min-w-0 items-center gap-3 rounded-md px-3 py-2 text-left text-sm hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => navigate(`/collections/${ref.collection}/${encodeURIComponent(ref.record_id)}`)}><span className="min-w-0 flex-1 truncate font-medium">{ref.collection}/{ref.record_id}</span><span className="truncate text-xs text-muted-foreground">{ref.path}</span><ChevronRight size={15} className="shrink-0 text-muted-foreground" /></button>)}</CardContent></Card>}
  </>;
}

function MediaIcon({ entry }: { entry: MediaEntry }) {
  if (entry.kind === 'directory') return <Folder size={17} />;
  if (/\.(png|jpe?g|gif|webp)$/i.test(entry.name)) return <img src={`/api/media/file?path=${encodeURIComponent(entry.path)}`} alt="" loading="lazy" className="size-8 rounded-sm object-cover" />;
  return <FileText size={17} />;
}

function MediaReferencesButton({ path }: { path: string }) {
  const [open, setOpen] = useState(false);
  const [references, setReferences] = useState<Reference[] | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    if (!open) return;
    let active = true;
    setReferences(null);
    api<Reference[]>(`/media/references?path=${encodeURIComponent(path)}`)
      .then(items => { if (active) { setReferences(items); setError(''); } })
      .catch(failure => { if (active) setError(errorMessage(failure)); });
    return () => { active = false; };
  }, [open, path]);
  return <><Tooltip><TooltipTrigger asChild><Button type="button" size="icon" variant="ghost" aria-label={`References to ${path}`} onClick={() => setOpen(true)}><Links size={16} /></Button></TooltipTrigger><TooltipContent side="top">References</TooltipContent></Tooltip>
    <Dialog open={open} onOpenChange={setOpen}><DialogContent><DialogHeader><DialogTitle>References to {path}</DialogTitle><DialogDescription className="sr-only">Records linking to this media file.</DialogDescription></DialogHeader>
      <div className="max-h-72 overflow-y-auto">{references?.map((reference, index) => <a key={`${reference.collection}/${reference.record_id}/${reference.path}/${index}`} href={`#/collections/${encodeURIComponent(reference.collection)}/${encodeURIComponent(reference.record_id)}`} onClick={() => setOpen(false)} className="flex min-w-0 items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent"><span className="min-w-0 flex-1 truncate">{reference.collection}/{reference.record_id}</span><span className="truncate text-xs text-muted-foreground">{reference.path}</span></a>)}{references && !references.length && <p className="py-2 text-sm text-muted-foreground">No references.</p>}{!references && !error && <p className="py-2 text-sm text-muted-foreground">Loading…</p>}{error && <p className="py-2 text-sm text-destructive">{error}</p>}</div>
    </DialogContent></Dialog></>;
}

function MediaPage({ editable, onError }: { editable: boolean; onError: (s: string) => void }) {
  const [folder, setFolder] = useState(''); const [entries, setEntries] = useState<MediaEntry[]>([]); const [offset, setOffset] = useState(0); const [version, setVersion] = useState(0); const [hasNext, setHasNext] = useState(false);
  const [action, setAction] = useState<{ kind: 'folder' | 'move'; entry?: MediaEntry } | null>(null); const [draftPath, setDraftPath] = useState(''); const [busy, setBusy] = useState(false);
  const refresh = () => setVersion(v => v+1);
  useEffect(() => { let active = true; api<MediaEntry[]>(`/media?path=${encodeURIComponent(folder)}&offset=${offset}&limit=51`).then(items => { if (active) { setEntries(items.slice(0,50)); setHasNext(items.length > 50); } }).catch(e => { if (active) onError(errorMessage(e)); }); return () => { active = false; }; }, [folder, offset, version]);
  async function upload(e: React.ChangeEvent<HTMLInputElement>) { const file = e.target.files?.[0]; if (!file) return; const body = new FormData(); body.append('directory',folder); body.append('file',file); try { await api('/media/upload',{ method:'POST', body }); refresh(); } catch(error) { onError(errorMessage(error)); } e.target.value = ''; }
  async function submitPath(e: React.SyntheticEvent<HTMLFormElement>) { e.preventDefault(); if (!action || !draftPath.trim()) return; setBusy(true); try { if (action.kind === 'folder') await api('/media/mkdir',{ method:'POST', body:jsonBody({ path: folder ? `${folder}/${draftPath.trim()}` : draftPath.trim() }) }); else if (action.entry && draftPath.trim() !== action.entry.path) await api('/media/move',{ method:'POST', body:jsonBody({ source:action.entry.path, target:draftPath.trim() }) }); setAction(null); refresh(); } catch(error) { onError(errorMessage(error)); } finally { setBusy(false); } }
  async function remove(entry: MediaEntry) { try { await api('/media/delete',{ method:'POST', body:jsonBody({ path:entry.path }) }); refresh(); } catch(e) { onError(errorMessage(e)); } }
  return <>
    <Heading title="Media">{editable && <div className="flex gap-2"><Button variant="outline" onClick={() => { setDraftPath(''); setAction({kind:'folder'}); }}><Folder size={16} /> New folder</Button><Button asChild><label htmlFor="media-upload"><Upload size={16} /> Upload</label></Button><input id="media-upload" type="file" className="sr-only" onChange={upload} /></div>}</Heading>
    {folder && <div className="mb-4 flex min-w-0 items-center gap-2 text-sm"><Button variant="ghost" size="sm" onClick={() => { setFolder(''); setOffset(0); }}>Media</Button><span className="text-muted-foreground">/</span><span className="min-w-0 flex-1 truncate text-muted-foreground">{folder}</span><Button variant="ghost" size="sm" onClick={() => { setFolder(folder.split('/').slice(0,-1).join('/')); setOffset(0); }}>Up</Button></div>}
    <Card><div className="space-y-0.5">{entries.map(entry => <div key={entry.path} className="flex min-w-0 flex-wrap items-center gap-2 rounded-md px-3 py-2 sm:flex-nowrap"><button type="button" className="flex min-w-0 flex-1 items-center gap-3 text-left hover:text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => entry.kind === 'directory' ? (setFolder(entry.path),setOffset(0)) : window.open(`/api/media/file?path=${encodeURIComponent(entry.path)}`,'_blank')}><span className="grid size-8 shrink-0 place-items-center rounded-md bg-secondary text-primary"><MediaIcon entry={entry} /></span><span className="min-w-0 truncate font-medium">{entry.name}</span></button><span className="text-xs text-muted-foreground">{entry.kind === 'file' ? `${Math.max(1,Math.round(entry.size/1024))} KB` : ''}</span>{entry.kind === 'file' && <MediaReferencesButton path={entry.path} />}{editable && <div className="flex items-center"><Button size="icon" variant="ghost" aria-label={`Move ${entry.name}`} title={`Move ${entry.name}`} onClick={() => { setDraftPath(entry.path); setAction({kind:'move',entry}); }}><MoveIcon size={16} /></Button><AlertDialog><AlertDialogTrigger asChild><Button size="icon" variant="ghost" className="text-destructive hover:text-destructive" aria-label={`Delete ${entry.name}`} title={`Delete ${entry.name}`}><Trash size={16} /></Button></AlertDialogTrigger><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>Delete {entry.name}?</AlertDialogTitle><AlertDialogDescription>This removes {entry.path} from the media folder.</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>Cancel</AlertDialogCancel><AlertDialogAction className="bg-destructive text-destructive-foreground hover:bg-destructive/90" onClick={() => remove(entry)}>Delete</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog></div>}</div>)}{!entries.length && <p className="px-3 py-6 text-sm text-muted-foreground">This directory is empty.</p>}</div></Card>
    {(offset > 0 || hasNext) && <div className="mt-4 flex items-center justify-between"><Button variant="outline" size="sm" disabled={offset === 0} onClick={() => setOffset(Math.max(0,offset-50))}>Previous</Button><span className="text-sm text-muted-foreground">{entries.length ? `${offset + 1}–${offset + entries.length}` : '0 items'}</span><Button variant="outline" size="sm" disabled={!hasNext} onClick={() => setOffset(offset+50)}>Next</Button></div>}
    <Dialog open={!!action} onOpenChange={open => { if (!open) setAction(null); }}><DialogContent><DialogHeader><DialogTitle>{action?.kind === 'folder' ? 'New folder' : 'Move media'}</DialogTitle><DialogDescription>{action?.kind === 'folder' ? `Create a folder inside /${folder}` : 'Enter a path relative to the media root.'}</DialogDescription></DialogHeader><form onSubmit={submitPath} className="space-y-4"><div className="space-y-2"><Label htmlFor="media-path">{action?.kind === 'folder' ? 'Folder name' : 'New path'}</Label><Input id="media-path" value={draftPath} onChange={e => setDraftPath(e.target.value)} required autoFocus /></div><DialogFooter><Button type="button" variant="outline" onClick={() => setAction(null)}>Cancel</Button><Button disabled={busy || !draftPath.trim()}>{busy ? 'Saving…' : action?.kind === 'folder' ? 'Create folder' : 'Move'}</Button></DialogFooter></form></DialogContent></Dialog>
  </>;
}

function AccountPage({ onDone, onError }: { onDone: () => void; onError: (s: string) => void }) {
  const [oldPassword, setOldPassword] = useState(''); const [newPassword, setNewPassword] = useState(''); const [busy, setBusy] = useState(false);
  async function submit(e: React.SyntheticEvent<HTMLFormElement>) { e.preventDefault(); setBusy(true); try { await api('/auth/password',{ method:'POST', body:jsonBody({ old_password:oldPassword, new_password:newPassword }) }); onDone(); } catch(err) { onError(errorMessage(err)); } finally { setBusy(false); } }
  return <><Heading title="Change password" /><Card className="max-w-xl"><CardContent className="pt-4"><form onSubmit={submit} className="space-y-4"><div className="space-y-2"><Label htmlFor="old-password">Current password</Label><Input id="old-password" type="password" autoComplete="current-password" value={oldPassword} onChange={e => setOldPassword(e.target.value)} required /></div><div className="space-y-2"><Label htmlFor="new-password">New password</Label><Input id="new-password" type="password" autoComplete="new-password" minLength={12} value={newPassword} onChange={e => setNewPassword(e.target.value)} required placeholder="At least 12 characters" /></div><Button disabled={busy}>{busy ? 'Updating…' : 'Update password'}</Button></form></CardContent></Card></>;
}

function AdminPage({ onError }: { onError: (s: string) => void }) {
  type Invitation = { id: number; email: string | null; role: User['role']; expires_at: number; redeemed: boolean; revoked: boolean };
  const [users, setUsers] = useState<User[]>([]); const [invites, setInvites] = useState<Invitation[]>([]); const [role, setRole] = useState<User['role']>('editor'); const [email, setEmail] = useState('');
  const [link, setLink] = useState<{kind:'Invitation'|'Password reset';url:string}|null>(null); const [copied, setCopied] = useState(false); const [busy, setBusy] = useState(false);
  const refresh = () => { api<User[]>('/admin/users').then(setUsers).catch(e => onError(errorMessage(e))); api<Invitation[]>('/admin/invitations').then(setInvites).catch(e => onError(errorMessage(e))); };
  useEffect(refresh, []);
  async function invite(e: React.SyntheticEvent<HTMLFormElement>) { e.preventDefault(); setBusy(true); try { const result = await api<{token:string}>('/admin/invitations',{ method:'POST', body:jsonBody({ role, email:email.trim() || null }) }); setLink({kind:'Invitation',url:`${location.origin}/#invite/${result.token}`}); setCopied(false); setEmail(''); refresh(); } catch(error) { onError(errorMessage(error)); } finally { setBusy(false); } }
  async function update(id: number, change: Partial<Pick<User,'role'|'disabled'>>) { try { await api(`/admin/users/${id}`,{ method:'PUT', body:jsonBody(change) }); refresh(); } catch(error) { onError(errorMessage(error)); } }
  async function reset(id: number) { try { const result = await api<{token:string}>(`/admin/users/${id}/reset`,{ method:'POST' }); setLink({kind:'Password reset',url:`${location.origin}/#reset/${result.token}`}); setCopied(false); } catch(error) { onError(errorMessage(error)); } }
  async function revokeSessions(id: number) { try { await api(`/admin/users/${id}/revoke-sessions`,{ method:'POST' }); } catch(error) { onError(errorMessage(error)); } }
  async function revoke(id: number) { try { await api(`/admin/invitations/${id}/revoke`,{ method:'POST' }); refresh(); } catch(error) { onError(errorMessage(error)); } }
  async function copyLink() { if (!link) return; try { await navigator.clipboard.writeText(link.url); setCopied(true); } catch(error) { onError(errorMessage(error)); } }
  return <><Heading title="Administration" /><div className="grid items-start gap-5 xl:grid-cols-[minmax(0,1.15fr)_minmax(0,0.85fr)]">
    <Card><CardHeader><CardTitle className="text-base">Users</CardTitle></CardHeader><CardContent className="space-y-0">{users.map(u => <div key={u.id} className="py-3"><div className="flex min-w-0 flex-wrap items-start justify-between gap-2"><div className="min-w-0"><p className="truncate font-medium">{u.display_name}</p><p className="truncate text-sm text-muted-foreground">{u.email}</p></div><Badge variant={u.disabled ? 'outline' : 'secondary'}>{u.disabled ? 'Disabled' : 'Active'}</Badge></div><div className="mt-3 flex flex-wrap items-center gap-2"><Select value={u.role} onValueChange={value => update(u.id,{role:value as User['role']})}><SelectTrigger aria-label={`Role for ${u.email}`} className="w-[150px]"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="user">User</SelectItem><SelectItem value="editor">Editor</SelectItem><SelectItem value="administrator">Administrator</SelectItem></SelectContent></Select><Button variant="outline" size="sm" onClick={() => update(u.id,{disabled:!u.disabled})}>{u.disabled ? 'Enable' : 'Disable'}</Button><Button variant="ghost" size="sm" onClick={() => reset(u.id)}>Reset link</Button><Button variant="ghost" size="sm" onClick={() => revokeSessions(u.id)}>Sign out all</Button></div></div>)}{!users.length && <p className="text-sm text-muted-foreground">No users yet.</p>}</CardContent></Card>
    <div className="min-w-0 space-y-5"><Card><CardHeader><CardTitle className="text-base">Create invitation</CardTitle></CardHeader><CardContent><form onSubmit={invite} className="space-y-4"><div className="space-y-2"><Label htmlFor="invite-email">Email address <span className="font-normal text-muted-foreground">(optional)</span></Label><Input id="invite-email" value={email} onChange={e => setEmail(e.target.value)} type="email" autoComplete="email" placeholder="person@example.com" /></div><div className="space-y-2"><Label htmlFor="invite-role">Role</Label><Select value={role} onValueChange={value => setRole(value as User['role'])}><SelectTrigger id="invite-role"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="user">User</SelectItem><SelectItem value="editor">Editor</SelectItem><SelectItem value="administrator">Administrator</SelectItem></SelectContent></Select></div><Button disabled={busy}>{busy ? 'Creating…' : 'Create link'}</Button></form>{link && <div className="mt-4 space-y-2"><p className="text-sm font-medium">{link.kind} link</p><Input aria-label={`${link.kind} link`} readOnly value={link.url} onFocus={e => e.target.select()} /><Button variant="outline" size="sm" onClick={copyLink}>{copied ? <Check size={15} /> : null}{copied ? 'Copied' : 'Copy link'}</Button></div>}</CardContent></Card>
    <Card><CardHeader><CardTitle className="text-base">Invitations</CardTitle></CardHeader><CardContent>{invites.length ? <div className="space-y-0">{invites.map(i => <div key={i.id} className="flex min-w-0 flex-wrap items-center justify-between gap-2 py-2"><div className="min-w-0"><p className="truncate text-sm font-medium">{i.email || 'Any email address'}</p><p className="text-xs text-muted-foreground">{i.role} · {i.redeemed ? 'Redeemed' : i.revoked ? 'Revoked' : `Expires ${new Date(i.expires_at*1000).toLocaleString()}`}</p></div>{!i.redeemed && !i.revoked && <Button size="sm" variant="ghost" onClick={() => revoke(i.id)}>Revoke</Button>}</div>)}</div> : <p className="text-sm text-muted-foreground">No invitations yet.</p>}</CardContent></Card></div>
  </div></>;
}
