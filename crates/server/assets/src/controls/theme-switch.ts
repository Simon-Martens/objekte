const colorPreference = matchMedia('(prefers-color-scheme: dark)');
function storedTheme() {
  try { return localStorage.getItem('objekte-theme'); } catch { return null; }
}
function preferredTheme() {
  const personal = storedTheme();
  const instance = document.getElementById('ui-preferences')?.dataset.defaultTheme;
  const choice = ['light', 'dark'].includes(personal ?? '') ? personal : instance;
  return choice === 'light' || choice === 'dark' ? choice : colorPreference.matches ? 'dark' : 'light';
}
function applyTheme(theme: string) {
  document.documentElement.dataset.theme = theme;
  document.querySelectorAll<HTMLButtonElement>('[data-theme-choice]').forEach(button => {
    button.setAttribute('aria-pressed', String(button.dataset.themeChoice === theme));
  });
  window.dispatchEvent(new CustomEvent('theme-change', { detail: theme }));
}
class ThemeSwitch extends HTMLElement {
  connectedCallback() {
    this.addEventListener('click', this.choose);
    applyTheme(document.documentElement.dataset.theme || 'light');
  }
  disconnectedCallback() { this.removeEventListener('click', this.choose); }
  private choose = (event: Event) => {
    const theme = (event.target as Element).closest<HTMLButtonElement>('[data-theme-choice]')?.dataset.themeChoice;
    if (theme !== 'light' && theme !== 'dark') return;
    try { localStorage.setItem('objekte-theme', theme); } catch { /* Session-only choice is still usable. */ }
    applyTheme(theme);
  };
}
customElements.define('theme-switch', ThemeSwitch);
colorPreference.addEventListener('change', () => applyTheme(preferredTheme()));
window.addEventListener('storage', event => {
  if (event.key === 'objekte-theme') applyTheme(preferredTheme());
});

window.addEventListener('preferences-change', () => applyTheme(preferredTheme()));
