/** Checkbox changes submit to Rust; keep the chooser usable after the list patch. */
import { openMenu } from './action-menu';
let pending: { form: HTMLFormElement; path: string; name: string; scroll: number } | undefined;
// A label's native focus/activation can scroll the page and dismiss the floating
// panel before it forwards the click. Activate its checkbox without scrolling.
document.addEventListener('pointerdown', event => {
  const target = event.target as Element;
  if (event.button === 0 && target.closest('.column-option') && !target.matches('input')) event.preventDefault();
});
document.addEventListener('click', event => {
  const target = event.target as Element;
  const label = target.closest<HTMLLabelElement>('.column-picker .column-option');
  if (!label || target.matches('input')) return;
  const input = label.querySelector<HTMLInputElement>('input[type="checkbox"]');
  event.preventDefault();
  if (!input || input.disabled || input.closest('[inert]')) return;
  input.focus({preventScroll:true});
  input.click();
});

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
  if (location.pathname !== previous.path) return;
  const menu = document.querySelector<HTMLDetailsElement>('.column-picker');
  if (!menu) return;
  requestAnimationFrame(() => {
    if (pending !== previous || !menu.isConnected || location.pathname !== previous.path) return;
    pending = undefined;
    openMenu(menu);
    const form = menu.querySelector('form')!;
    form.scrollTop = previous.scroll;
    form.querySelector<HTMLInputElement>(`[name="${CSS.escape(previous.name)}"]`)?.focus({ preventScroll: true });
  });
});
document.addEventListener('pointerdown', event => {
  if (!(event.target as Element).closest('.column-picker') || (event.target as Element).closest('[data-close-menu], .column-picker > summary')) pending = undefined;
});
document.addEventListener('keydown', event => { if (event.key === 'Escape') pending = undefined; });
window.addEventListener('navigate', () => { pending = undefined; });
