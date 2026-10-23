const preferenceKey = 'objekte-confirm-discard';
let enabled = true;
try { enabled = localStorage.getItem(preferenceKey) !== 'false'; } catch { /* Storage may be unavailable. */ }
function syncOption() {
  document.querySelectorAll<HTMLInputElement>('#record-form input[name="confirm_discard"]').forEach(input => { input.value = String(enabled); });
  document.querySelectorAll<HTMLInputElement>('[data-confirm-discard]').forEach(input => { input.checked = enabled; });
}
new MutationObserver(syncOption).observe(document.body, {childList: true, subtree: true});
syncOption();
document.addEventListener('change', event => {
  const input = event.target;
  if (!(input instanceof HTMLInputElement) || !input.matches('[data-confirm-discard]')) return;
  enabled = input.checked;
  syncOption();
  try { localStorage.setItem(preferenceKey, String(enabled)); } catch { /* Keep the in-memory preference. */ }
});
window.addEventListener('storage', event => {
  if (event.key !== preferenceKey && event.key !== null) return;
  enabled = event.newValue !== 'false';
  syncOption();
});
