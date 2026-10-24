/** URLs identify records; tab storage holds private draft identifiers. */
import { projectDraftKey } from './project-chooser';
const project = document.querySelector<HTMLElement>('[data-project-id]')?.dataset.projectId ?? 'default';
const prefix = 'objekte:draft:' + projectDraftKey(project, '');
const inputPrefix = 'objekte:draft-input:';
const claims = new Map<string, {request:string; busy:()=>void}>();
const channel = typeof BroadcastChannel === 'undefined' ? null : new BroadcastChannel('objekte-drafts');
const tab = crypto.randomUUID();
const pending = new Map<string, () => void>();
channel?.addEventListener('message', ({data}) => {
  if (data.tab === tab) return;
  if (data.kind === 'claim' && document.querySelector<HTMLElement>('#workspace')?.dataset.draftId === data.id) {
    channel.postMessage({kind:'busy', request:data.request, tab});
  } else if (data.kind === 'claim' && claims.has(data.id)) {
    if (tab < data.tab) channel?.postMessage({kind:'busy', request:data.request, tab});
    else claims.get(data.id)?.busy();
  } else if (data.kind === 'busy') pending.get(data.request)?.();
});
function key(url: string) {
  const parsed = new URL(url, location.origin);
  // Keep server escaping stable (URLSearchParams otherwise changes encoding).
  const query = parsed.search.slice(1).split('&').filter(p => p && !p.startsWith('node=')).join('&');
  return parsed.pathname + (query ? '?' + query : '');
}
function read(k: string) { try { return sessionStorage.getItem(prefix+k) ?? ''; } catch { return ''; } }
function forget(id: string, inputs = true) {
  if (inputs) { try { sessionStorage.removeItem(inputPrefix + id); } catch {} }
  try { for (const k of Object.keys(sessionStorage)) if (k.startsWith(prefix) && sessionStorage.getItem(k) === id) sessionStorage.removeItem(k); } catch { /* Storage may be disabled. */ }
}
async function available(id: string) {
  if (!id || !channel || document.querySelector<HTMLElement>('#workspace')?.dataset.draftId === id) return id;
  const request = crypto.randomUUID();
  let busy = false;
  pending.set(request, () => { busy = true; });
  claims.set(id, {request, busy:() => { busy = true; }});
  channel.postMessage({kind:'claim', id, request, tab});
  await new Promise(resolve => setTimeout(resolve, 120));
  pending.delete(request);
  claims.delete(id);
  return busy ? '' : id;
}
function formKey(form: HTMLFormElement) {
  const action = new URL(form.action, location.origin).pathname;
  if (!action.endsWith('/edit-sessions')) return null;
  if (action.startsWith('/documents/')) {
    const file = (form.elements.namedItem('file') as HTMLInputElement)?.value;
    if (!file) return null;
    // Match Rust's percent encoding of non-alphanumeric bytes.
    const encoded = Array.from(new TextEncoder().encode(file), b => /[a-zA-Z0-9]/.test(String.fromCharCode(b)) ? String.fromCharCode(b) : '%' + b.toString(16).toUpperCase().padStart(2,'0')).join('');
    return action.replace(/edit-sessions$/, 'edit') + '?file=' + encoded;
  }
  if (action.includes('/records/')) return action.replace(/edit-sessions$/, 'edit');
  return form.hasAttribute('data-draft-entry') ? action.replace(/edit-sessions$/, 'new') : null;
}
const prepared = new WeakSet<HTMLFormElement>();
document.addEventListener('submit', async event => {
  const form = event.target as HTMLFormElement;
  const k = formKey(form);
  if (!k || prepared.has(form)) { prepared.delete(form); return; }
  event.preventDefault();
  event.stopImmediatePropagation();
  const submitter = (event as SubmitEvent).submitter as HTMLButtonElement | null;
  const id = form.dataset.expired === 'true' ? '' : await available(read(k));
  let input = form.querySelector<HTMLInputElement>('input[name="draft"]');
  if (!input) { input = document.createElement('input'); input.type='hidden'; input.name='draft'; form.append(input); }
  input.value = id;
  let restore = form.querySelector<HTMLInputElement>('input[name="restore"]');
  if (!restore) { restore = document.createElement('input'); restore.type='hidden'; restore.name='restore'; form.append(restore); }
  try { restore.value = id ? sessionStorage.getItem(inputPrefix+id) ?? '' : ''; } catch { restore.value=''; }
  prepared.add(form);
  form.requestSubmit(submitter);
}, true);
const started = new WeakSet<HTMLFormElement>();
let lastClosed = '';
const shownDialogs = new WeakSet<HTMLDialogElement>();
export function syncDraft() {
  const workspace = document.querySelector<HTMLElement>('#workspace');
  if (!workspace) return;
  const dialog = workspace.querySelector<HTMLDialogElement>('#draft-close-dialog');
  if (dialog && !shownDialogs.has(dialog)) {
    shownDialogs.add(dialog);
    dialog.close();
    dialog.showModal();
    dialog.addEventListener('cancel', event => {
      event.preventDefault();
      dialog.querySelector<HTMLButtonElement>('button[formaction$="/keep-editing"]')?.click();
    });
  }
  const {draftId, draftKey, closedDraft, navigateTo, expiredDraft} = workspace.dataset;
  if (draftId && draftKey) {
    try { forget(draftId, false); sessionStorage.setItem(prefix+draftKey, draftId); } catch { /* Optional refresh recovery. */ }
  }
  if (draftId) {
    try {
      const saved = JSON.parse(sessionStorage.getItem(inputPrefix+draftId) ?? 'null');
      const version = workspace.querySelector<HTMLInputElement>('input[name="version"]')?.value;
      if (saved && saved.version !== version) sessionStorage.removeItem(inputPrefix+draftId);
    } catch { sessionStorage.removeItem(inputPrefix+draftId); }
  }
  if (closedDraft && navigateTo && lastClosed !== closedDraft) {
    lastClosed = closedDraft;
    forget(closedDraft);
    window.dispatchEvent(new CustomEvent('navigate', {detail:{url:navigateTo}}));
  }
  const form = workspace.querySelector<HTMLFormElement>('form[data-draft-entry]');
  if (form?.dataset.expired === 'true' || expiredDraft === 'true') {
    try { const k = key(workspace.dataset.url ?? location.href); forget(read(k)); sessionStorage.removeItem(prefix + k); } catch { /* Optional storage. */ }
  }
  if (form && !started.has(form)) {
    started.add(form);
    if (form.dataset.expired !== 'true') setTimeout(() => { if (form.isConnected) form.requestSubmit(); }, 0);
  }
}

// Preserve input not yet submitted, including invalid values, across a refresh.
document.addEventListener('input', event => {
  const form = (event.target as Element)?.closest<HTMLFormElement>('#record-form');
  const id = document.querySelector<HTMLElement>('#workspace')?.dataset.draftId;
  if (!form || !id) return;
  const values = Object.fromEntries(Array.from(new FormData(form).entries()).filter((entry): entry is [string,string] => typeof entry[1] === 'string'));
  delete values.csrf;
  try { sessionStorage.setItem(inputPrefix+id, JSON.stringify(values)); } catch { /* The unload warning still protects unsent input. */ }
});
