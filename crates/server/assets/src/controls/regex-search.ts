import { validatePattern } from './search-validator';

type SearchState = { version: number; validated?: string; timer?: ReturnType<typeof setTimeout> };
const states = new WeakMap<HTMLFormElement, SearchState>();
function state(form: HTMLFormElement): SearchState {
  let value = states.get(form);
  if (!value) { value = { version: 0 }; states.set(form, value); }
  return value;
}
function fields(form: HTMLFormElement) {
  return {
    input: form.querySelector<HTMLInputElement>('input[name="q"]')!,
    mode: form.querySelector<HTMLInputElement>('input[name="regex"]')!,
    error: form.closest('.table-panel')?.querySelector<HTMLElement>('#search-pattern-error'),
  };
}
function feedback(form: HTMLFormElement, message: string) {
  const { input, error } = fields(form);
  input.setCustomValidity(message);
  input.setAttribute('aria-invalid', String(Boolean(message)));
  if (error) error.textContent = message;
}
async function validate(form: HTMLFormElement): Promise<boolean> {
  const current = state(form);
  clearTimeout(current.timer);
  const version = ++current.version;
  const { input, mode } = fields(form);
  const key = mode.value + ':' + input.value;
  let message = '';
  try {
    if (mode.value === '1') message = await validatePattern(input.value);
  } catch {
    // The library still validates on submission if the browser cannot load WASM.
    if (version === current.version && form.isConnected) {
      feedback(form, '');
      if (fields(form).error) fields(form).error!.textContent = 'Pattern validation is unavailable. Search will validate it on the server.';
      current.validated = key;
      return true;
    }
    return false;
  }
  if (version !== current.version || !form.isConnected) return false;
  feedback(form, message);
  current.validated = key;
  return !message;
}
document.addEventListener('input', event => {
  const target = event.target;
  if (!(target instanceof HTMLInputElement) || target.name !== 'q') return;
  const form = target.closest<HTMLFormElement>('form[data-regex-search]');
  if (!form) return;
  const current = state(form);
  current.version++;
  current.validated = undefined;
  clearTimeout(current.timer);
  feedback(form, '');
  current.timer = setTimeout(() => { void validate(form); }, 120);
});
document.addEventListener('click', event => {
  const button = (event.target as Element)?.closest<HTMLButtonElement>('[data-regex-toggle]');
  const form = button?.closest<HTMLFormElement>('form[data-regex-search]');
  if (!button || !form) return;
  const { input, mode } = fields(form);
  mode.value = mode.value === '1' ? '0' : '1';
  button.setAttribute('aria-pressed', String(mode.value === '1'));
  feedback(form, '');
  void validate(form);
  input.focus();
});
// Run before Datastar's form handler. Never submit an unvalidated pattern while
// asynchronous WASM loading or an earlier validation is still pending.
document.addEventListener('submit', event => {
  const form = event.target;
  if (!(form instanceof HTMLFormElement) || !form.matches('[data-regex-search]')) return;
  const { input, mode } = fields(form);
  if (state(form).validated === mode.value + ':' + input.value && input.validity.valid) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  void validate(form).then(valid => { if (valid) form.requestSubmit(); });
}, true);
