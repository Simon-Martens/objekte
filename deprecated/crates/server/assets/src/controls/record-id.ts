/** Convenience only: normal server validation still applies when checking/saving. */
document.addEventListener('click', event => {
  const button = (event.target as Element)?.closest<HTMLButtonElement>('[data-add-uuid]');
  if (!button || button.disabled || button.closest('[inert]')) return;
  const input = document.querySelector<HTMLInputElement>('#record-id');
  if (!input || input.disabled || input.readOnly) return;
  const uuid = crypto.randomUUID();
  input.value = input.value ? `${input.value}-${uuid}` : uuid;
  input.dispatchEvent(new Event('input', {bubbles: true}));
  input.focus();
  input.setSelectionRange(input.value.length, input.value.length);
});
