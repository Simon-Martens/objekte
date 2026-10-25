/** Related tables load independently and use the same search/column controls as lists. */
export function installRecordReferences(
  win: Window & typeof globalThis,
  openMenu: (menu: HTMLDetailsElement) => void = menu => { menu.open = true; },
) {
  class ReferenceLoader extends win.HTMLElement {
    static observedAttributes = ['data-url'];
    private request?: AbortController;
    private url = '';
    private source = '';
    private attempted = false;

    attributeChangedCallback() {
      win.queueMicrotask(() => {
        if (this.isConnected && this.source !== this.dataset.url) {
          this.source = this.dataset.url ?? '';
          this.innerHTML = '';
          this.hidden = true;
          void this.load(this.source);
        }
      });
    }

    connectedCallback() {
      this.source = this.dataset.url ?? '';
      this.url ||= this.source;
      this.onclick = event => {
        const target = event.target as Element;
        if (target.closest('record-references, record-reference-table') !== this) return;
        const link = target.closest<HTMLAnchorElement>('a[data-reference-query]');
        const refresh = target.closest('[data-reference-refresh], [data-reference-retry]');
        if (!link && !refresh) return;
        event.preventDefault();
        event.stopPropagation();
        void this.load(link?.href ?? this.url, link?.hasAttribute('data-reference-page'));
      };
      this.onsubmit = event => {
        const form = event.target;
        if (!(form instanceof win.HTMLFormElement) || !form.matches('[data-reference-query]')
          || form.closest('record-references, record-reference-table') !== this || event.defaultPrevented) return;
        event.preventDefault();
        event.stopPropagation();
        const url = new win.URL(form.action);
        new win.FormData(form).forEach((value, key) => { if (typeof value === 'string') url.searchParams.append(key, value); });
        void this.load(url.href, false, form);
      };
      // The outer loader checks quietly. Individual tables already arrive with
      // server-rendered rows and fetch only when their own view changes.
      if (this.localName === 'record-references' && !this.attempted) void this.load(this.source);
    }

    disconnectedCallback() {
      if (this.request) { this.request.abort(); this.attempted = false; }
      this.request = undefined;
    }

    private async load(url: string, pageChange = false, form?: HTMLFormElement) {
      if (!url || !this.isConnected) return;
      this.request?.abort();
      const controller = new win.AbortController();
      const source = this.dataset.url;
      const menu = form?.closest<HTMLDetailsElement>('.column-picker');
      const active = win.document.activeElement as HTMLInputElement | null;
      const focusName = active?.name;
      const selection = active instanceof win.HTMLInputElement && active.type === 'search'
        ? [active.selectionStart, active.selectionEnd] : undefined;
      const menuScroll = form?.scrollTop ?? 0;
      this.request = controller;
      this.attempted = true;
      this.url = url;
      this.setAttribute('aria-busy', 'true');
      try {
        const response = await win.fetch(url, { signal: controller.signal, headers: { Accept: 'text/html' } });
        if (!response.ok || response.redirected) throw new Error('References request failed');
        const html = await response.text();
        if (!this.isConnected || controller.signal.aborted || source !== this.dataset.url) return;
        const restoreMenu = menu?.open && menu.isConnected;
        const restoreFocus = form?.contains(win.document.activeElement);
        this.innerHTML = html;
        this.hidden = !html.trim();
        const root = this.parentElement?.closest<HTMLElement>('record-references');
        if (root) root.hidden = ![...root.querySelectorAll<HTMLElement>('record-reference-table')].some(table => !table.hidden);
        if (restoreMenu && menu) {
          const replacement = win.document.getElementById(menu.id) as HTMLDetailsElement | null;
          if (replacement) {
            openMenu(replacement);
            replacement.querySelector('form')!.scrollTop = menuScroll;
          }
        }
        if (pageChange && !this.hidden) {
          this.querySelector<HTMLElement>('h2')?.focus({ preventScroll: true });
          this.scrollIntoView({ block: 'start' });
        } else if (restoreFocus && focusName) {
          const input = [...this.querySelectorAll<HTMLInputElement>('input')].find(input => input.name === focusName);
          input?.focus({ preventScroll: true });
          if (input?.type === 'search' && selection) input.setSelectionRange(selection[0], selection[1]);
        }
      } catch {
        if (!this.isConnected || controller.signal.aborted || source !== this.dataset.url) return;
        // Keep existing rows usable if a refresh fails. An initial failure
        // remains hidden because no links have been confirmed yet.
        this.querySelector('.reference-state')?.remove();
        this.insertAdjacentHTML('afterbegin', '<div class="reference-state" role="status"><p>Linked records could not be loaded.</p><button type="button" class="small" data-reference-retry>Try again</button></div>');
      } finally {
        if (this.request === controller) { this.removeAttribute('aria-busy'); this.request = undefined; }
      }
    }
  }
  win.customElements.define('record-references', class extends ReferenceLoader {});
  win.customElements.define('record-reference-table', class extends ReferenceLoader {});
}
