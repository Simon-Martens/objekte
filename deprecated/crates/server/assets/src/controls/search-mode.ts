/** The server compiles patterns; the client schedules checks and displays feedback. */
export {};
let timer: ReturnType<typeof setTimeout> | undefined;
let pending: AbortController | undefined;

function cancelValidation() {
  clearTimeout(timer);
  pending?.abort();
  pending = undefined;
}

function showFeedback(input: HTMLInputElement, error: string | null) {
  const feedback = input.form?.querySelector<HTMLElement>('[data-search-feedback]');
  if (!feedback) return;
  feedback.textContent = error ? `Invalid regular expression: ${error}` : '';
  feedback.hidden = !error;
  if (error) {
    input.setAttribute('aria-invalid', 'true');
    input.setAttribute('aria-describedby', feedback.id);
  } else {
    input.removeAttribute('aria-invalid');
    input.removeAttribute('aria-describedby');
  }
  // A submitted pattern's error no longer describes the text being edited.
  const submittedError = input.closest('.table-panel')?.querySelector<HTMLElement>('#search-error');
  if (submittedError) submittedError.hidden = true;
}

function validateInput(event: Event) {
  const input = event.target;
  if (!(input instanceof HTMLInputElement) || !input.matches('[data-search-pattern]')) return;
  cancelValidation();
  showFeedback(input, null);
  const form = input.form;
  const mode = form?.querySelector<HTMLInputElement>('input[name="regex"]');
  if (!form?.dataset.searchValidation || mode?.value !== '1' || !input.value
    || (event instanceof InputEvent && event.isComposing)) return;
  const query = input.value;
  timer = setTimeout(async () => {
    if (!input.isConnected) return;
    const controller = new AbortController();
    pending = controller;
    try {
      const response = await fetch(`${form.dataset.searchValidation}?${new URLSearchParams({q: query})}`, {
        signal: controller.signal,
        credentials: 'same-origin',
        headers: {Accept: 'application/json'},
      });
      if (!response.ok) return;
      const result: {error: string | null} = await response.json();
      if (controller.signal.aborted || pending !== controller || !input.isConnected
        || input.value !== query || mode.value !== '1') return;
      showFeedback(input, result.error);
    } catch {
      // Aborted/offline checks are not pattern errors. Submission still validates.
    } finally {
      if (pending === controller) pending = undefined;
    }
  }, 150);
}

document.addEventListener('input', validateInput);
document.addEventListener('compositionend', validateInput);
document.addEventListener('submit', cancelValidation, true);
window.addEventListener('navigate', cancelValidation);
window.addEventListener('popstate', cancelValidation);

document.addEventListener('click', event => {
  const button = (event.target as Element).closest<HTMLButtonElement>('[data-search-regex], [data-search-case]');
  const form = button?.form;
  if (!button || !form) return;
  const matchCase = button.matches('[data-search-case]');
  const regexMode = form.querySelector<HTMLInputElement>('input[name="regex"]');
  const mode = matchCase ? form.querySelector<HTMLInputElement>('input[name="case_sensitive"]') : regexMode;
  if (!mode || (matchCase && regexMode?.value === '1')) return;
  cancelValidation();
  const input = form.querySelector<HTMLInputElement>('[data-search-pattern]');
  if (input) showFeedback(input, null);
  const enabled = button.getAttribute('aria-pressed') !== 'true';
  button.setAttribute('aria-pressed', String(enabled));
  mode.value = enabled ? '1' : '0';
  if (!matchCase) {
    const caseButton = form.querySelector<HTMLButtonElement>('[data-search-case]');
    if (caseButton) caseButton.hidden = enabled;
    input?.closest('.search-input')?.classList.toggle('has-search-case', !enabled);
  }
  form.requestSubmit();
});
