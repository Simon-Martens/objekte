/** Checkbox changes submit to Rust; keep the chooser usable after the list patch. */
import { openMenu } from './action-menu';
let pending: { form: HTMLFormElement; path: string; name: string; scroll: number } | undefined;
document.addEventListener('submit', event => {
  const form = event.target;
  if (!(form instanceof HTMLFormElement) || !form.matches('.column-picker form')) return;
  pending = {
    form,
    path: location.pathname,
    name: (document.activeElement as HTMLInputElement)?.name ?? '',
    scroll: form.scrollTop,
  };
}, true);
document.addEventListener('datastar-fetch', event => {
  const { type, el } = (event as CustomEvent<{ type: string; el: Element }>).detail;
  if (type !== 'finished' || pending?.form !== el) return;
  const previous = pending;
  pending = undefined;
  if (location.pathname !== previous.path) return;
  const menu = document.querySelector<HTMLDetailsElement>('.column-picker');
  if (!menu) return;
  requestAnimationFrame(() => {
    if (!menu.isConnected || location.pathname !== previous.path) return;
    openMenu(menu);
    const form = menu.querySelector('form')!;
    form.scrollTop = previous.scroll;
    form.querySelector<HTMLInputElement>(`[name="${CSS.escape(previous.name)}"]`)?.focus({ preventScroll: true });
  });
});
document.addEventListener('pointerdown', event => {
  if (!(event.target as Element).closest('.column-picker')) pending = undefined;
});
document.addEventListener('keydown', event => { if (event.key === 'Escape') pending = undefined; });
window.addEventListener('navigate', () => { pending = undefined; });
