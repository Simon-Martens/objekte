/** Floating geometry and dismissal only; menu content and actions come from Rust. */
const selector = 'details.action-menu, details.media-references';
let active: HTMLDetailsElement | undefined;
let panel: HTMLElement | undefined;
const resize = new ResizeObserver(() => { if (active) position(); });

function close(restoreFocus = false) {
  resize.disconnect();
  const previous = active;
  if (panel?.isConnected && panel.matches(':popover-open')) panel.hidePopover();
  active = undefined;
  panel = undefined;
  if (previous) {
    previous.open = false;
    if (restoreFocus) previous.querySelector<HTMLElement>('summary')?.focus();
  }
}

function position() {
  if (!active?.isConnected || !panel?.isConnected) { close(); return; }
  const anchor = active.querySelector('summary')!.getBoundingClientRect();
  const margin = 12, gap = 6;
  const width = document.documentElement.clientWidth;
  const height = window.innerHeight;
  const below = height - anchor.bottom - margin - gap;
  const above = anchor.top - margin - gap;
  const upward = panel.scrollHeight > below && above > below;
  panel.style.maxHeight = `${Math.max(0, upward ? above : below)}px`;
  const bounds = panel.getBoundingClientRect();
  const left = active.classList.contains('add-field-picker') ? anchor.left : anchor.right - bounds.width;
  panel.style.left = `${Math.max(margin, Math.min(left, width - bounds.width - margin))}px`;
  panel.style.top = `${upward ? Math.max(margin, anchor.top - bounds.height - gap) : anchor.bottom + gap}px`;
}

export function openMenu(menu: HTMLDetailsElement) {
  if (active === menu && panel?.matches(':popover-open')) return;
  close();
  menu.open = true;
  active = menu;
  panel = menu.querySelector<HTMLElement>(':scope > .menu-panel, :scope > div') ?? undefined;
  if (!panel) return;
  // The top layer prevents tables and scroll containers from clipping menus.
  panel.setAttribute('popover', 'manual');
  panel.showPopover();
  position();
  resize.observe(panel);
}

document.addEventListener('toggle', event => {
  const menu = event.target;
  if (!(menu instanceof HTMLDetailsElement) || !menu.matches(selector)) return;
  if (!menu.open) { if (active === menu) close(); return; }
  openMenu(menu);
}, true);

document.addEventListener('pointerdown', event => {
  if (active && !active.contains(event.target as Node)) close();
});
document.addEventListener('keydown', event => {
  if (event.key === 'Escape' && active) {
    event.preventDefault();
    close(true);
  }
});
document.addEventListener('focusin', event => {
  if (active && !active.contains(event.target as Node)) close();
});
document.addEventListener('click', event => {
  if ((event.target as Element).closest('[data-copy]')) close();
  if ((event.target as Element).closest('[data-close-menu]')) close(true);
});
window.addEventListener('navigate', () => close());
window.addEventListener('resize', () => { if (active) position(); });
document.addEventListener('scroll', event => {
  if (active && !panel?.contains(event.target as Node)) {
    if (active.classList.contains('column-picker')) position();
    else close();
  }
}, true);
