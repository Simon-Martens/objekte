/** Native inputs/Monaco own typing history; Rust owns structural draft history. */
let localHistory = false;
let version = '';
document.addEventListener('input', event => {
  if ((event.target as Element)?.closest('#record-form')) localHistory = true;
});
new MutationObserver(() => {
  const next = document.querySelector<HTMLFormElement>('#record-form')?.dataset.version || '';
  if (next !== version) { version = next; localHistory = false; }
}).observe(document.body, {subtree:true, childList:true, attributes:true, attributeFilter:['data-version']});
function hasLocalInput(form: HTMLFormElement) {
  return [...form.querySelectorAll<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>('[name="record_id"], [name="raw"], [name^="value."]')].some(control => {
    if (control instanceof HTMLInputElement && control.type === 'checkbox') return control.checked !== control.defaultChecked;
    if (control instanceof HTMLSelectElement) return [...control.options].some(option => option.selected !== option.defaultSelected);
    return control.value !== (control instanceof HTMLTextAreaElement ? control.textContent : control.getAttribute('value'));
  });
}
document.addEventListener('keydown', event => {
  if (!(event.ctrlKey || event.metaKey) || event.altKey || event.isComposing || document.querySelector('server-picker')) return;
  const key = event.key.toLowerCase();
  if (key !== 'z' && key !== 'y') return;
  const form = document.querySelector<HTMLFormElement>('#record-form');
  if (!form || form.querySelector('fieldset[inert]')) return;
  const redo = key === 'y' || event.shiftKey;
  const target = event.target as HTMLElement;
  const inInput = !!target.closest('code-input, input:not([type="checkbox"]):not([type="radio"]), textarea, [contenteditable="true"]');
  if (inInput && (hasLocalInput(form) || redo && localHistory)) return;
  const button = document.querySelector<HTMLButtonElement>(`[data-history="${redo ? 'redo' : 'undo'}"]`);
  if (button && (!button.disabled || !redo && hasLocalInput(form))) {
    event.preventDefault(); event.stopImmediatePropagation();
    // Checkboxes/selects have no native undo stack. The command submits their latest
    // value first, so Rust can restore the preceding snapshot even on a fresh draft.
    button.disabled = false;
    button.click();
  }
}, true);
