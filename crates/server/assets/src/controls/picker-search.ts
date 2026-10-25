/** Read-only search: it never submits the record, advances its version, or moves focus. */
export function installPickerSearch(win: Window & typeof globalThis) {
  const doc = win.document;
  const jobs = new WeakMap<HTMLElement, { timer?: number; controller?: AbortController }>();
  const search = (picker: HTMLElement, immediate = false) => {
    let job = jobs.get(picker);
    if (!job) { job = {}; jobs.set(picker, job); }
    win.clearTimeout(job.timer);
    job.controller?.abort();
    const run = async () => {
      const input = picker.querySelector<HTMLInputElement>('input[name="picker_query"]');
      const form = picker.closest('form');
      if (!input || !form || !picker.isConnected) return;
      if (picker.closest('[inert]')) { job!.timer = win.setTimeout(run, 50); return; }
      const query = input.value, version = form.dataset.version;
      const controller = new win.AbortController(); job!.controller = controller;
      picker.setAttribute('aria-busy', 'true');
      try {
        const response = await win.fetch(`${picker.dataset.searchUrl}?q=${encodeURIComponent(query)}`, {signal:controller.signal, headers:{Accept:'text/html'}});
        if (!response.ok) throw new Error('Search failed');
        const html = await response.text();
        if (controller.signal.aborted || !picker.isConnected || input.value !== query) return;
        if (form.dataset.version !== version || picker.closest('[inert]')) { search(picker, true); return; }
        const content = new win.DOMParser().parseFromString(html, 'text/html').querySelector('[data-picker-options]');
        if (!content) throw new Error('Missing search results');
        picker.querySelector('[data-picker-options]')?.replaceWith(doc.importNode(content, true));
        picker.querySelector('[data-search-error]')?.remove();
      } catch (error) {
        if (controller.signal.aborted || !picker.isConnected) return;
        let notice = picker.querySelector<HTMLElement>('[data-search-error]');
        if (!notice) { notice = doc.createElement('p'); notice.dataset.searchError = ''; notice.setAttribute('role','alert'); picker.querySelector('.picker-search')?.after(notice); }
        notice.textContent = input.dataset.searchError ?? 'Search could not be loaded. Try again.';
      } finally { if (job!.controller === controller) picker.removeAttribute('aria-busy'); }
    };
    job.timer = win.setTimeout(run, immediate ? 0 : 180);
  };
  doc.addEventListener('input', event => {
    const input = event.target;
    if (!(input instanceof win.HTMLInputElement) || input.name !== 'picker_query' || event instanceof win.InputEvent && event.isComposing) return;
    const picker = input.closest<HTMLElement>('server-picker[data-search-url]');
    if (picker) search(picker);
  });
  doc.addEventListener('click', event => {
    const button = (event.target as Element)?.closest('[data-picker-search]');
    const picker = button?.closest<HTMLElement>('server-picker[data-search-url]');
    if (picker) { event.preventDefault(); search(picker, true); }
  });
  doc.addEventListener('submit', event => {
    const form = event.target;
    if (!(form instanceof win.HTMLFormElement)) return;
    const picker = form.querySelector<HTMLElement>('server-picker[data-search-url]');
    if (!picker) return;
    const job = jobs.get(picker);
    win.clearTimeout(job?.timer); job?.controller?.abort();
    picker.removeAttribute('aria-busy');
  }, true);
}
