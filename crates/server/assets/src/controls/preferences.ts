let last = '';
function apply() {
  const node = document.getElementById('ui-preferences');
  if (!node || node.outerHTML === last) return;
  last = node.outerHTML;
  document.documentElement.dataset.density = node.dataset.density;
  document.documentElement.style.setProperty('--editor-font-size', `${node.dataset.editorFont}px`);
  window.dispatchEvent(new CustomEvent('preferences-change', {detail: {...node.dataset}}));
}
new MutationObserver(apply).observe(document.body, {childList:true, subtree:true, attributes:true, attributeFilter:['data-density','data-editor-font','data-validation-delay','data-default-theme']});
queueMicrotask(apply);
document.addEventListener('click', async event => {
  const button = (event.target as Element)?.closest<HTMLButtonElement>('[data-copy]');
  if (!button) return;
  try { await navigator.clipboard.writeText(button.dataset.copy || ''); button.setAttribute('aria-label','Copied'); button.title='Copied'; } catch { button.title='Select and copy the value manually'; }
});
// Disclosure menus share predictable outside-click and Escape behavior.
document.addEventListener('click', event => {
  document.querySelectorAll<HTMLDetailsElement>('details.action-menu[open]').forEach(menu => {
    if (!menu.contains(event.target as Node)) menu.open=false;
  });
});
document.addEventListener('keydown', event => {
  if (event.key !== 'Escape') return;
  document.querySelectorAll<HTMLDetailsElement>('details.action-menu[open]').forEach(menu => {
    menu.open=false;menu.querySelector<HTMLElement>('summary')?.focus();
  });
});
