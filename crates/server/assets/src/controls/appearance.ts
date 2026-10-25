/** Personal appearance overrides project defaults without changing project data. */
export function installAppearance(doc: Document, win: Window & typeof globalThis) {
  const colors = win.matchMedia('(prefers-color-scheme: dark)');
  const read = (key: string) => { try { return win.localStorage.getItem(key); } catch { return null; } };
  let theme = read('objekte-theme');
  let density = read('objekte-density');
  const choice = (value: string | null, choices: string[]) => choices.includes(value ?? '') ? value! : 'default';
  const save = (key: string, value: string) => {
    try {
      if (value === 'default') win.localStorage.removeItem(key);
      else win.localStorage.setItem(key, value);
    } catch { /* The choice still works for this page when storage is unavailable. */ }
  };
  function apply() {
    const defaults = doc.getElementById('ui-preferences')?.dataset;
    const themeChoice = choice(theme, ['system', 'light', 'dark']);
    const densityChoice = choice(density, ['compact', 'comfortable']);
    const requested = themeChoice === 'default' ? defaults?.defaultTheme : themeChoice;
    const effectiveTheme = requested === 'light' || requested === 'dark' ? requested : colors.matches ? 'dark' : 'light';
    const effectiveDensity = densityChoice === 'default' ? defaults?.density ?? 'compact' : densityChoice;
    if (doc.documentElement.dataset.theme !== effectiveTheme) {
      doc.documentElement.dataset.theme = effectiveTheme;
      win.dispatchEvent(new win.CustomEvent('theme-change', { detail: effectiveTheme }));
    }
    if (doc.documentElement.dataset.density !== effectiveDensity) doc.documentElement.dataset.density = effectiveDensity;
    doc.querySelectorAll<HTMLButtonElement>('[data-theme-choice]').forEach(button => {
      // Auto follows the project's setting; explicit choices stay independently selectable.
      button.setAttribute('aria-pressed', String(button.dataset.themeChoice === themeChoice));
    });
    doc.querySelectorAll<HTMLSelectElement>('[data-personal-theme]').forEach(select => { select.value = themeChoice; });
    doc.querySelectorAll<HTMLSelectElement>('[data-personal-density]').forEach(select => { select.value = densityChoice; });
  }
  function announce(message: string) {
    doc.querySelectorAll<HTMLElement>('[data-appearance-status]').forEach(node => { node.textContent = message; });
  }
  doc.addEventListener('click', event => {
    const target = event.target as Element;
    const reset = target.closest('[data-reset-appearance]');
    const next = target.closest<HTMLButtonElement>('[data-theme-choice]')?.dataset.themeChoice;
    if (reset) {
      theme = density = 'default';
      save('objekte-theme', theme); save('objekte-density', density);
      announce('Using project appearance.');
    } else if (next && ['default', 'light', 'dark'].includes(next)) {
      theme = next; save('objekte-theme', theme);
      announce('Appearance saved for this browser.');
    } else return;
    apply();
  });
  doc.addEventListener('change', event => {
    const select = event.target as HTMLSelectElement;
    if (select.matches('[data-personal-theme]')) {
      theme = choice(select.value, ['system', 'light', 'dark']); save('objekte-theme', theme);
    } else if (select.matches('[data-personal-density]')) {
      density = choice(select.value, ['compact', 'comfortable']); save('objekte-density', density);
    } else return;
    apply(); announce('Appearance saved for this browser.');
  });
  win.addEventListener('storage', event => {
    if (event.key === 'objekte-theme' || event.key === null) theme = read('objekte-theme');
    if (event.key === 'objekte-density' || event.key === null) density = read('objekte-density');
    apply();
  });
  colors.addEventListener('change', apply);
  win.addEventListener('preferences-change', apply);
  // Navigation replaces server fragments, including the settings controls.
  new win.MutationObserver(apply).observe(doc.body, {childList: true, subtree: true});
  apply();
}
