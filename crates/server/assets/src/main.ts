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
import './controls/action-menu';
import './controls/list-columns';
import './controls/column-resize';
import './controls/request-feedback';
import './controls/tooltips';
import './controls/code-input';
import { confirmNavigation } from './controls/navigation-dialog';
import { syncDraft } from './controls/draft-lifecycle';

// Navigation mechanics only. Datastar applies every server-generated DOM patch.
let dirty = false;
let version = '';
let shownUrl = location.pathname + location.search;
function syncLocation() {
  syncDraft();
  const workspace = document.getElementById('workspace');
  const url = workspace?.dataset.url;
  const nextVersion = workspace?.querySelector<HTMLInputElement>('input[name="version"], input[name="revision"]')?.value ?? '';
  if (url && url.split('?')[0] !== shownUrl.split('?')[0]) {
    window.scrollTo({top:0,left:0,behavior:'instant'});
    workspace?.focus({preventScroll:true});
  }
  if (url) shownUrl = url;
  if (nextVersion !== version) { dirty = false; version = nextVersion; }
  if (url && url.startsWith('/') && !url.startsWith('//') && location.pathname + location.search !== url) {
    if (location.pathname.includes('/edit-sessions')) history.replaceState(null, '', url);
    else history.pushState(null, '', url);
    dirty = false;
  }
}
new MutationObserver(syncLocation).observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-url', 'value'] });
syncLocation();
document.addEventListener('input', event => { if ((event.target as Element)?.closest('#record-form, #document-form')) dirty = true; });
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
