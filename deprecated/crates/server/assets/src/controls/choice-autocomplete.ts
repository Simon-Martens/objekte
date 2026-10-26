/** Token autocomplete. Query text is never a form value; only server choices can be added. */
export function installChoiceAutocomplete(win: Window & typeof globalThis, choices: {values(field: HTMLElement): unknown[]; add(field: HTMLElement, value: unknown, label: string): boolean}) {
  const doc = win.document;
  let input: HTMLInputElement | undefined;
  let popup: HTMLElement | undefined;
  let controller: AbortController | undefined;
  let timer: number | undefined;
  let active = -1;
  const options = () => [...(popup?.querySelectorAll<HTMLButtonElement>('[role=option]') ?? [])];
  const close = () => {
    win.clearTimeout(timer); controller?.abort();
    input?.setAttribute('aria-expanded', 'false');
    input?.removeAttribute('aria-activedescendant');
    input?.removeAttribute('aria-busy');
    popup?.remove(); popup = undefined; input = undefined; active = -1;
  };
  const position = () => {
    if (!input?.isConnected || !popup?.isConnected) { close(); return; }
    const box = input.closest('[data-select-anchor]')!.getBoundingClientRect();
    const width = Math.min(Math.max(box.width, 240), win.innerWidth - 24);
    const below = win.innerHeight - box.bottom - 12, above = box.top - 12;
    const flip = below < 180 && above > below;
    Object.assign(popup.style, {
      width: `${width}px`, left: `${Math.max(12, Math.min(box.left, win.innerWidth - width - 12))}px`,
      maxHeight: `${Math.max(80, Math.min(300, flip ? above : below))}px`,
      top: flip ? 'auto' : `${box.bottom + 4}px`,
      bottom: flip ? `${win.innerHeight - box.top + 4}px` : 'auto',
    });
  };
  const highlight = (index: number) => {
    const buttons = options(); active = buttons.length ? (index + buttons.length) % buttons.length : -1;
    buttons.forEach((button, n) => button.setAttribute('aria-selected', String(n === active)));
    if (active < 0) input?.removeAttribute('aria-activedescendant');
    else { input?.setAttribute('aria-activedescendant', buttons[active].id); buttons[active].scrollIntoView?.({block:'nearest'}); }
  };
  const search = (target: HTMLInputElement, immediate = false) => {
    if (input !== target) { close(); input = target; }
    win.clearTimeout(timer); controller?.abort();
    // Never let Enter choose a result from an older query.
    popup?.remove(); popup = undefined; active = -1;
    target.removeAttribute('aria-activedescendant'); target.setAttribute('aria-expanded', 'false');
    const run = async () => {
      if (input !== target || !target.isConnected || doc.activeElement !== target) return;
      if (target.closest('[inert]')) { timer = win.setTimeout(run, 50); return; }
      const form = target.closest('form')!, version = form.dataset.version, query = target.value;
      const request = new win.AbortController(); controller = request;
      target.setAttribute('aria-busy','true');
      try {
        const response = await win.fetch(`${target.dataset.searchUrl}?q=${encodeURIComponent(query)}`, {signal:request.signal, headers:{Accept:'text/html'}});
        if (!response.ok) throw new Error('Search failed');
        const html = await response.text();
        if (request.signal.aborted || !target.isConnected || input !== target || target.value !== query) return;
        if (form.dataset.version !== version || target.closest('[inert]')) { search(target, true); return; }
        const results = new win.DOMParser().parseFromString(html, 'text/html').querySelector('[data-picker-options]');
        if (!results) throw new Error('Missing choices');
        popup = doc.createElement('div'); popup.className = 'choice-suggestions';
        popup.id = target.getAttribute('aria-controls')!; popup.setAttribute('role','listbox');
        popup.setAttribute('aria-label',target.closest('[data-select-anchor]')!.getAttribute('aria-label')!);
        const field = target.closest<HTMLElement>('[data-local-choices]')!;
        const selected = choices.values(field).map(value => JSON.stringify(value));
        const matches = [...results.querySelectorAll<HTMLButtonElement>('button[name=choice]')].filter(button => !selected.includes(JSON.stringify(JSON.parse(button.value))));
        const prefix = query.trim().toLocaleLowerCase();
        const rank = (button: HTMLButtonElement) => button.textContent?.trim().toLocaleLowerCase().startsWith(prefix) ? 0 : 1;
        matches.sort((a, b) => rank(a) - rank(b));
        for (const source of matches) {
          const button = doc.importNode(source, true);
          button.id = `${popup.id}-${popup.children.length}`; button.setAttribute('role','option');
          button.removeAttribute('aria-checked'); button.tabIndex = -1;
          button.type = 'button'; button.removeAttribute('formaction');
          button.addEventListener('click', () => {
            if (choices.add(field, JSON.parse(button.value), button.querySelector('span')?.textContent ?? button.textContent ?? '')) {
              close(); target.value = ''; target.focus({preventScroll:true});
            }
          });
          button.querySelector('i')?.remove(); popup.append(button);
        }
        if (!popup.children.length) {
          const notice = doc.createElement('p'); notice.setAttribute('role','status');
          notice.textContent = target.dataset.noMatches!; popup.append(notice);
        }
        target.closest('[data-select-anchor]')!.append(popup);
        target.setAttribute('aria-expanded','true'); position(); highlight(0);
      } catch {
        if (request.signal.aborted || input !== target || !target.isConnected) return;
        popup = doc.createElement('div'); popup.className = 'choice-suggestions';
        popup.setAttribute('role','alert'); popup.textContent = target.dataset.searchError!;
        target.closest('[data-select-anchor]')!.append(popup); position();
      } finally { if (controller === request) target.removeAttribute('aria-busy'); }
    };
    timer = win.setTimeout(run, immediate ? 0 : 180);
  };
  doc.addEventListener('input', event => {
    const target = event.target;
    if (target instanceof win.HTMLInputElement && target.matches('[data-choice-input]')) {
      if (event instanceof win.InputEvent && event.isComposing) { close(); return; }
      search(target);
    }
  });
  doc.addEventListener('compositionend', event => {
    const target = event.target;
    if (target instanceof win.HTMLInputElement && target.matches('[data-choice-input]')) search(target);
  });
  doc.addEventListener('keydown', event => {
    const target = event.target;
    if (!(target instanceof win.HTMLInputElement) || !target.matches('[data-choice-input]') || event.isComposing) return;
    if (event.key === 'Enter') { event.preventDefault(); if (input === target) options()[active]?.click(); }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!popup || input !== target) search(target, true);
      else highlight(active + (event.key === 'ArrowDown' ? 1 : -1));
    }
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); }
    if (event.key === 'Tab') close();
  });
  // Keep the input focused while clicking a suggestion.
  doc.addEventListener('pointerdown', event => {
    if (popup?.contains(event.target as Node)) event.preventDefault();
    else if (event.target !== input) close();
  });
  doc.addEventListener('focusout', () => {
    win.setTimeout(() => { if (input && doc.activeElement !== input && !popup?.contains(doc.activeElement) && !input.closest('[inert]')) close(); }, 0);
  });
  doc.addEventListener('submit', close, true);
  win.addEventListener('resize',position);
  win.addEventListener('scroll', () => { if (popup) position(); }, true);
}
