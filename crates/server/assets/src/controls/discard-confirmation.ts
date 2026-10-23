const preferenceKey = 'objekte-confirm-discard';
let enabled = true;
try { enabled = localStorage.getItem(preferenceKey) !== 'false'; } catch { /* Storage may be unavailable. */ }
function syncOption() {
  document.querySelectorAll<HTMLInputElement>('[data-confirm-discard]').forEach(input => { input.checked = enabled; });
}
new MutationObserver(syncOption).observe(document.body, {childList: true, subtree: true});
syncOption();
document.addEventListener('change', event => {
  const input = event.target;
  if (!(input instanceof HTMLInputElement) || !input.matches('[data-confirm-discard]')) return;
  enabled = input.checked;
  try { localStorage.setItem(preferenceKey, String(enabled)); } catch { /* Keep the in-memory preference. */ }
});
window.addEventListener('storage', event => {
  if (event.key !== preferenceKey && event.key !== null) return;
  enabled = event.newValue !== 'false';
  syncOption();
});
const confirmed = new WeakSet<HTMLElement>();
document.addEventListener('submit', async event => {
  const button = event.submitter;
  const form = event.target;
  if (!(button instanceof HTMLButtonElement) || !(form instanceof HTMLFormElement)) return;
  if (confirmed.delete(button)) return;
  if (!enabled || !/\/edit-sessions\/[^/]+\/discard(?:-close)?$/.test(button.getAttribute('formaction') ?? '')) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  const dialog = document.querySelector<HTMLDialogElement>('#discard-dialog');
  let discard = false;
  if (dialog) {
    if (dialog.open) return;
    discard = await new Promise<boolean>(resolve => {
      dialog.returnValue = 'cancel';
      dialog.addEventListener('close', () => resolve(dialog.returnValue === 'discard'), {once: true});
      dialog.showModal();
    });
  } else {
    discard = window.confirm('Discard your unsaved changes?');
  }
  if (!discard || !form.isConnected || !button.isConnected || button.form !== form) return;
  confirmed.add(button);
  form.requestSubmit(button);
  confirmed.delete(button);
}, true);
