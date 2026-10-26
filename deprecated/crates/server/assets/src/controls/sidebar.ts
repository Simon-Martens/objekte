export {};

// Layout state belongs to the shell; repository navigation remains server-owned.
const shell = document.getElementById('admin-shell');
const toggle = document.querySelector<HTMLButtonElement>('[data-sidebar-toggle]');
const divider = document.querySelector<HTMLElement>('[data-sidebar-resizer]');
const mobile = window.matchMedia('(max-width: 720px)');
const storageKey = 'objekte-sidebar-layout';
let width = 232;
let collapsed = false;
let dragging: number | undefined;

try {
  const saved = JSON.parse(localStorage.getItem(storageKey) ?? '{}');
  if (typeof saved.width === 'number' && Number.isFinite(saved.width)) width = saved.width;
  collapsed = saved.collapsed === true;
} catch { /* Storage may be unavailable; layout still works for this page. */ }

function maximum() { return Math.min(520, Math.max(180, window.innerWidth - (mobile.matches ? 48 : 360))); }
function apply() {
  width = Math.round(Math.max(180, Math.min(maximum(), width)));
  shell?.style.setProperty('--sidebar-width', `${width}px`);
  shell?.classList.toggle('sidebar-collapsed', collapsed);
  const visible = mobile.matches ? shell?.classList.contains('navigation-open') : !collapsed;
  toggle?.setAttribute('aria-expanded', String(!!visible));
  toggle?.setAttribute('aria-label', visible ? 'Hide sidebar' : 'Show sidebar');
  divider?.setAttribute('aria-valuenow', String(width));
  divider?.setAttribute('aria-valuemax', String(maximum()));
}
function save() {
  try { localStorage.setItem(storageKey, JSON.stringify({ width, collapsed })); } catch { /* Optional persistence. */ }
}
function closeMobile() {
  shell?.classList.remove('navigation-open');
  apply();
}
toggle?.addEventListener('click', () => {
  if (mobile.matches) shell?.classList.toggle('navigation-open');
  else collapsed = !collapsed;
  apply();
  save();
});
divider?.addEventListener('pointerdown', event => {
  if (event.button !== 0 || mobile.matches) return;
  event.preventDefault();
  dragging = event.pointerId;
  divider.setPointerCapture(event.pointerId);
  shell?.classList.add('sidebar-resizing');
});
divider?.addEventListener('pointermove', event => {
  if (event.pointerId !== dragging) return;
  width = event.clientX - (shell?.getBoundingClientRect().left ?? 0);
  apply();
});
function endDrag() {
  dragging = undefined;
  shell?.classList.remove('sidebar-resizing');
  save();
}
divider?.addEventListener('pointerup', endDrag);
divider?.addEventListener('pointercancel', endDrag);
divider?.addEventListener('lostpointercapture', endDrag);
divider?.addEventListener('keydown', event => {
  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
  event.preventDefault();
  if (event.key === 'Home') width = 180;
  else if (event.key === 'End') width = maximum();
  else width += (event.key === 'ArrowRight' ? 1 : -1) * (event.shiftKey ? 40 : 10);
  apply();
  save();
});
window.addEventListener('resize', apply);
mobile.addEventListener('change', closeMobile);
window.addEventListener('navigate', closeMobile);
document.addEventListener('keydown', event => { if (event.key === 'Escape') closeMobile(); });
apply();
