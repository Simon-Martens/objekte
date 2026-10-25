// Schedules read-only server validation. Datastar alone patches the returned feedback.
let timer: ReturnType<typeof setTimeout> | undefined;
let pending: AbortController | undefined;
function cancel() { clearTimeout(timer); pending?.abort(); }
document.addEventListener('input', event => {
  if ((event.target as Element)?.closest('server-picker, [data-choice-input]')) return;
  const target = event.target;
  if (!(target instanceof HTMLElement)) return;
  const form = target.closest<HTMLFormElement>('form[data-live-validation]');
  if (!form) return;
  cancel();
  timer = setTimeout(() => {
    if (!form.isConnected) return;
    pending = new AbortController();
    form.querySelector('[data-validation-request]')?.dispatchEvent(new CustomEvent('validate-draft', { detail: { controller: pending } }));
  }, Number(document.getElementById('ui-preferences')?.dataset.validationDelay || 400));
});
// Cancelling at the start of a new edit prevents older feedback arriving during typing.
document.addEventListener('submit', cancel, true);
window.addEventListener('navigate', cancel);
window.addEventListener('popstate', cancel);
