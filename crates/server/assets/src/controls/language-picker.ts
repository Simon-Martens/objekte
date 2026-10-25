// Switching language in an editor submits its current input into the existing
// draft. Nothing is saved to disk, and the normal version guard still applies.
document.addEventListener('change', event => {
  const select = event.target;
  if (!(select instanceof HTMLSelectElement) || !select.matches('[data-language-picker]')) return;
  const record = document.querySelector<HTMLFormElement>('#record-form');
  if (record) {
    if (record.querySelector('[inert]')) return;
    let input = record.querySelector<HTMLInputElement>('[name="ui_language"]');
    if (!input) { input = document.createElement('input'); input.type = 'hidden'; input.name = 'ui_language'; record.append(input); }
    input.value = select.value;
    const button = document.createElement('button'); button.hidden = true;
    button.setAttribute('formaction', record.action.replace(/\/save$/, '/language'));
    record.append(button); record.requestSubmit(button); button.remove();
  } else {
    const form = select.form;
    if (form) { (form.elements.namedItem('return_to') as HTMLInputElement).value = location.pathname + location.search; form.requestSubmit(); }
  }
});
new MutationObserver(() => {
  const select = document.querySelector<HTMLSelectElement>('[data-language-picker]');
  if (select) document.documentElement.lang = select.value;
}).observe(document.body, {childList:true,subtree:true});
