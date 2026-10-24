/** Load the server-rendered backlink panel independently of record navigation. */
export function installRecordReferences(win: Window & typeof globalThis) {
  class RecordReferences extends win.HTMLElement {
    static observedAttributes = ['data-url'];
    private request?: AbortController;
    private url = '';
    private source = '';

    attributeChangedCallback() {
      win.queueMicrotask(() => {
        if (this.isConnected && this.source !== this.dataset.url) {
          this.source = this.dataset.url ?? '';
          void this.load(this.source);
        }
      });
    }

    connectedCallback() {
      this.onclick = event => {
        const page = (event.target as Element).closest<HTMLAnchorElement>('a[data-reference-page]');
        const retry = (event.target as Element).closest('[data-reference-retry]');
        if (!page && !retry) return;
        event.preventDefault();
        void this.load(page?.href ?? this.url);
      };
      this.source = this.dataset.url ?? '';
      void this.load(this.source);
    }

    disconnectedCallback() { this.request?.abort(); }

    private async load(url: string) {
      if (!url || !this.isConnected) return;
      this.request?.abort();
      const controller = new win.AbortController();
      const source = this.dataset.url;
      this.request = controller;
      this.url = url;
      this.setAttribute('aria-busy', 'true');
      try {
        const response = await win.fetch(url, {
          signal: controller.signal,
          headers: { Accept: 'text/html' },
        });
        if (!response.ok || response.redirected) throw new Error('References request failed');
        const html = await response.text();
        if (!this.isConnected || controller.signal.aborted || source !== this.dataset.url) return;
        this.innerHTML = html;
      } catch {
        if (!this.isConnected || controller.signal.aborted || source !== this.dataset.url) return;
        this.innerHTML = '<p role="status">Incoming references could not be loaded. <button type="button" class="button quiet small" data-reference-retry>Retry</button></p>';
      } finally {
        if (this.request === controller) this.removeAttribute('aria-busy');
      }
    }
  }
  win.customElements.define('record-references', RecordReferences);
}
