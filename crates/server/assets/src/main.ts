import 'vite/modulepreload-polyfill';
import './vendor/datastar.js';
import '@fontsource-variable/source-sans-3';
import '@fontsource-variable/source-sans-3/wght-italic.css';
import '@fontsource/jetbrains-mono/latin-400.css';
import 'remixicon/fonts/remixicon.css';
import './style.css';
import './controls/theme-switch';
import './controls/live-validation';
import './controls/server-picker';
import { installPickerSearch } from './controls/picker-search';
installPickerSearch(window);
import { installChoiceAutocomplete } from './controls/choice-autocomplete';
import { installChoiceValues } from './controls/choice-values';
installChoiceAutocomplete(window, installChoiceValues(window));
import './controls/language-picker';
import { installArrayReorder } from './controls/array-reorder';
installArrayReorder(window);
import { installEditTableSearch } from './controls/edit-table-search';
installEditTableSearch(document);
import './controls/action-menu';
import { installColumnControls } from './controls/list-columns';
import { openMenu } from './controls/action-menu';
installColumnControls(document, openMenu);
import './controls/column-resize';
import './controls/request-feedback';
import './controls/tooltips';
import './controls/code-input';
import { confirmNavigation } from './controls/navigation-dialog';
import { syncDraft } from './controls/draft-lifecycle';

import { installProjectChooser } from './controls/project-chooser';
// Pin requests to the project rendered in this document, including older tabs.
const projectId = document.querySelector<HTMLElement>('[data-project-id]')?.dataset.projectId;
const nativeFetch = window.fetch.bind(window);
window.fetch = (input, init) => {
  const url = new URL(input instanceof Request ? input.url : String(input), location.href);
  if (!projectId || url.origin !== location.origin) return nativeFetch(input, init);
  const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined));
  headers.set('x-objekte-project', projectId);
  return nativeFetch(input, {...init, headers});
};
import { installRecordReferences } from './controls/record-references';
installRecordReferences(window, openMenu);

// Navigation mechanics only. Datastar applies every server-generated DOM patch.
let dirty = false;
installProjectChooser(document, async () => {
  if ((dirty || document.querySelector<HTMLElement>('#workspace')?.dataset.draftDirty === 'true') && !await confirmNavigation()) return false;
  dirty = false;
  return true;
}, form => form.requestSubmit());
let version = '';
let shownUrl = location.pathname + location.search;
let shownTablePage = document.getElementById('workspace')?.dataset.tablePage ?? '0';
let anchoredElement: HTMLElement | null = null;
function syncLocation() {
  syncDraft();
  const workspace = document.getElementById('workspace');
  const url = workspace?.dataset.url;
  const nextVersion = workspace?.querySelector<HTMLInputElement>('input[name="version"], input[name="revision"]')?.value ?? '';
  const locationKey = (value: string) => {
    const parsed = new URL(value, location.origin);
    return parsed.pathname + ':' + (parsed.searchParams.get('node') ?? '');
  };
  const tablePage = workspace?.dataset.tablePage ?? '0';
  if (url && (locationKey(url) !== locationKey(shownUrl) || tablePage !== shownTablePage)) {
    window.scrollTo({top:0,left:0,behavior:'instant'});
    workspace?.focus({preventScroll:true});
  }
  if (url) shownUrl = url;
  shownTablePage = tablePage;
  if (nextVersion !== version) { dirty = false; version = nextVersion; }
  if (url && url.startsWith('/') && !url.startsWith('//') && location.pathname + location.search !== url) {
    if (location.pathname.includes('/edit-sessions')) history.replaceState(null, '', url);
    else history.pushState(null, '', url);
    dirty = false;
  }
  // A record edit link can open an expired draft first. Scroll once the
  // replacement editor inserts its table card into the document.
  const anchor = location.hash.slice(1);
  const target = anchor ? document.getElementById(anchor) : null;
  if (target && target !== anchoredElement) {
    anchoredElement = target;
    requestAnimationFrame(() => target.isConnected && target.scrollIntoView({block:'start'}));
  }
}
new MutationObserver(syncLocation).observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-url', 'data-table-page', 'value'] });
syncLocation();
document.addEventListener('input', event => { if ((event.target as Element)?.closest('#record-form, #document-form') && !(event.target as Element)?.closest('server-picker, [data-choice-input]')) dirty = true; });
document.addEventListener('click', async event => {
  const link = (event.target as Element)?.closest<HTMLAnchorElement>('a[data-nav]');
  if (!link || event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
  event.preventDefault();
  if ((dirty || document.querySelector<HTMLElement>('#workspace')?.dataset.draftDirty === 'true') && !await confirmNavigation()) return;
  dirty = false;
  window.dispatchEvent(new CustomEvent('navigate', { detail: { url: link.pathname + link.search } }));
});
window.addEventListener('popstate', async () => {
  if ((dirty || document.querySelector<HTMLElement>('#workspace')?.dataset.draftDirty === 'true') && !await confirmNavigation()) {
    history.pushState(null, '', shownUrl); return;
  }
  dirty = false;
  window.dispatchEvent(new CustomEvent('navigate', { detail: { url: location.pathname + location.search } }));
});
window.addEventListener('beforeunload', event => { if (dirty) { event.preventDefault(); event.returnValue = ''; } });

import './controls/sidebar';

document.addEventListener('keydown', event => {
  if (!(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== 's') return;
  const save = document.querySelector<HTMLButtonElement>('button.primary[form="record-form"], #document-form button[value="save"]');
  if (save && !document.querySelector('server-picker')) { event.preventDefault(); save.click(); }
}, true);

import './controls/draft-history';
import './controls/preferences';
import './controls/navigation-tree';

import './controls/record-id';

import './controls/notifications';

import './controls/discard-confirmation';

import './controls/suggestion-select';

import { installSearchControls } from './controls/regex-search';
import { validatePattern } from './controls/search-validator';
installSearchControls(document, validatePattern);

// GET forms do not reliably retain fragments, so table controls carry an
// explicit anchor while preserving the other tables' query state.
const tableAnchor = new URLSearchParams(location.search).get('table_anchor');
if (tableAnchor && /^array-[0-9a-f]*$/.test(tableAnchor)) {
  const scrollToTable = () => document.getElementById(tableAnchor)?.scrollIntoView({block:'start'});
  if (document.readyState === 'complete') scrollToTable();
  else window.addEventListener('load', scrollToTable, {once:true});
}
