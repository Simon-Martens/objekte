type SearchState = { version: number; validated?: string; timer?: number };
type PatternValidator = (pattern: string, caseSensitive: boolean) => Promise<string>;

export function installSearchControls(document: Document, validatePattern: PatternValidator): void {
  const window = document.defaultView!;
  const { HTMLInputElement, HTMLFormElement, Element } = window;
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
      matchCase: form.querySelector<HTMLInputElement>('input[name="case_sensitive"]')!,
      error: form.closest('.table-panel')?.querySelector<HTMLElement>('#search-pattern-error'),
    };
  }
  function key(form: HTMLFormElement): string {
    const { input, mode, matchCase } = fields(form);
    return mode.value + ':' + matchCase.value + ':' + input.value;
  }
  function feedback(form: HTMLFormElement, message: string) {
    const { input, error } = fields(form);
    input.setCustomValidity(message);
    input.setAttribute('aria-invalid', String(Boolean(message)));
    if (error) error.textContent = message;
  }
  async function validate(form: HTMLFormElement): Promise<boolean> {
    const current = state(form);
    window.clearTimeout(current.timer);
    const version = ++current.version;
    const { input, mode, matchCase } = fields(form);
    const query = key(form);
    // Navigation can morph a connected form without firing input events.
    const isCurrent = () => version === current.version && form.isConnected && key(form) === query;
    let message = '';
    try {
      if (mode.value === '1') message = await validatePattern(input.value, matchCase.value === '1');
    } catch {
      // The library still validates on submission if the browser cannot load WASM.
      if (!isCurrent()) return false;
      feedback(form, '');
      const { error } = fields(form);
      if (error) error.textContent = 'Pattern validation is unavailable. Search will validate it on the server.';
      current.validated = query;
      return true;
    }
    if (!isCurrent()) return false;
    feedback(form, message);
    current.validated = query;
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
    window.clearTimeout(current.timer);
    feedback(form, '');
    current.timer = window.setTimeout(() => { void validate(form); }, 120);
  });
  document.addEventListener('click', event => {
    if (!(event.target instanceof Element)) return;
    const button = event.target.closest<HTMLButtonElement>('[data-regex-toggle], [data-case-toggle]');
    const form = button?.closest<HTMLFormElement>('form[data-regex-search]');
    if (!button || !form) return;
    const { input, mode, matchCase } = fields(form);
    const option = button.hasAttribute('data-case-toggle') ? matchCase : mode;
    option.value = option.value === '1' ? '0' : '1';
    button.setAttribute('aria-pressed', String(option.value === '1'));
    feedback(form, '');
    void validate(form);
    input.focus();
  });
  // Run before Datastar's form handler. Never submit an unvalidated pattern while
  // asynchronous WASM loading or an earlier validation is still pending.
  document.addEventListener('submit', event => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement) || !form.matches('[data-regex-search]')) return;
    const { input } = fields(form);
    if (state(form).validated === key(form) && input.validity.valid) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    void validate(form).then(valid => {
      if (!valid) return;
      const validated = state(form).validated;
      // Browsers can ignore requestSubmit while the original submit is still
      // dispatching, including its microtasks. Resume on the next event turn.
      window.setTimeout(() => {
        if (form.isConnected && validated === key(form) && state(form).validated === validated) form.requestSubmit();
      }, 0);
    });
  }, true);
}
