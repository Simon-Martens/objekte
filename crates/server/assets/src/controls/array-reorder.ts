/** Moves and removals update form state immediately; Rust reconciles IDs on submit. */
export function installArrayReorder(win: Window & typeof globalThis) {
  const doc = win.document;
  const rows = (array: HTMLElement) => [...array.querySelectorAll<HTMLElement>('[data-array-item]')].filter(row => row.closest('[data-array]') === array);
  let drag: { handle: HTMLButtonElement; row: HTMLElement; array: HTMLElement; x: number; y: number; active: boolean; target?: HTMLElement; after?: boolean } | undefined;
  function clear() {
    drag?.row.classList.remove('array-dragging');
    drag?.target?.classList.remove('array-drop-before', 'array-drop-after');
    drag = undefined;
  }
  const order = (array: HTMLElement) => JSON.parse(array.dataset.arrayOrder ?? '[]') as string[];
  function change(array: HTMLElement, ids: string[], removed = false) {
    array.dataset.arrayOrder = JSON.stringify(ids);
    let hidden = array.querySelector<HTMLInputElement>('[data-structure-values]');
    if (!hidden) {
      hidden = doc.createElement('input'); hidden.type = 'hidden';
      hidden.dataset.structureValues = ''; hidden.name = `structure.${array.dataset.structureArray}`;
      array.append(hidden);
    }
    hidden.value = JSON.stringify(ids);
    const visible = rows(array);
    for (const row of visible) {
      const index = ids.indexOf(row.dataset.arrayItem ?? '');
      const handle = row.querySelector<HTMLButtonElement>('[data-array-handle]');
      if (handle) {
        handle.dataset.up = index > 0 ? `${array.dataset.array}/up?item=${row.dataset.arrayItem}` : '';
        handle.dataset.down = index + 1 < ids.length ? `${array.dataset.array}/down?item=${row.dataset.arrayItem}` : '';
        handle.setAttribute('aria-label', (array.dataset.reorderLabel ?? 'Reorder item {number}').replace('{number}', String(index + 1)));
      }
      const position = row.querySelector<HTMLElement>('.position-cell');
      if (position) position.textContent = String(index + 1);
      if (row.matches('.array-item')) row.setAttribute('aria-label', `Item ${index + 1}`);
    }
    const badge = array.querySelector<HTMLElement>('.embedded-edit-toolbar .badge');
    if (badge) badge.textContent = String(ids.length);
    const range = array.querySelector<HTMLElement>('[data-table-range]');
    if (range) {
      const filtered = array.dataset.arrayFiltered === 'true';
      const results = filtered ? Math.max(0, Number(array.dataset.arrayResults ?? 0) - (removed ? 1 : 0)) : ids.length;
      if (filtered) array.dataset.arrayResults = String(results);
      const start = visible.length ? Number(array.dataset.arrayOffset ?? 0) + 1 : 0;
      range.textContent = (range.dataset.tableRange ?? '{start}–{end} of {total}')
        .replace('{start}',String(start)).replace('{end}',String(visible.length ? start + visible.length - 1 : 0)).replace('{total}',String(results));
    }
    const empty = array.querySelector<HTMLElement>('.array-empty');
    if (!visible.length && array.matches('.array-items') && !empty) {
      const notice = doc.createElement('p'); notice.className = 'array-empty muted';
      notice.textContent = array.dataset.emptyMessage ?? 'No items';
      array.insertBefore(notice,array.querySelector('button[formaction$="/add"]'));
    } else if (visible.length) empty?.remove();
    array.dispatchEvent(new win.Event('input',{bubbles:true}));
    const undo = doc.querySelector<HTMLButtonElement>('[data-history=undo]');
    if (undo) undo.disabled = false;
  }
  function localMove(array: HTMLElement, item: string, before: string) {
    const ids = order(array), from = ids.indexOf(item);
    if (from < 0 || before === item) return;
    ids.splice(from,1);
    const to = before ? ids.indexOf(before) : ids.length;
    if (to < 0) return;
    ids.splice(to,0,item);
    const row = rows(array).find(row => row.dataset.arrayItem === item);
    const destination = rows(array).find(row => row.dataset.arrayItem === before);
    if (row) {
      const parent = row.parentElement!;
      parent.insertBefore(row,destination ?? (array.matches('.array-items') ? array.querySelector('button[formaction$="/add"]') : null));
      row.querySelector<HTMLElement>('[data-array-handle]')?.focus({preventScroll:true});
    }
    change(array,ids);
  }
  function submit(handle: HTMLButtonElement, url: string, before = '') {
    const form = handle.form;
    const rowId = handle.closest<HTMLElement>('[data-array-item]')?.dataset.arrayItem;
    if (!form || !url || handle.matches(':disabled') || handle.closest('[inert]')) return;
    const array = handle.closest<HTMLElement>('[data-local-choices]');
    if (array && rowId) {
      if (url.includes('/up?') || url.includes('/down?')) {
        const items = rows(array), index = items.findIndex(row => row.dataset.arrayItem === rowId);
        before = url.includes('/up?') ? items[index - 1]?.dataset.arrayItem ?? '' : items[index + 2]?.dataset.arrayItem ?? '';
      }
      array.dispatchEvent(new win.CustomEvent('choice-move', {bubbles:true, detail:{item:rowId,before}}));
      return;
    }
    const structure = handle.closest<HTMLElement>('[data-structure-array]');
    if (structure && rowId) {
      if (url.includes('/up?') || url.includes('/down?')) {
        const ids = order(structure), index = ids.indexOf(rowId);
        before = url.includes('/up?') ? ids[index - 1] ?? '' : ids[index + 2] ?? '';
      }
      localMove(structure,rowId,before);
      return;
    }
    const version = form.dataset.version;
    let input = form.querySelector<HTMLInputElement>('[name="array_before"]');
    if (!input) { input = doc.createElement('input'); input.type = 'hidden'; input.name = 'array_before'; form.append(input); }
    input.value = before;
    let button = form.querySelector<HTMLButtonElement>('[data-array-command]');
    if (!button) { button = doc.createElement('button'); button.hidden = true; button.dataset.arrayCommand = ''; form.append(button); }
    button.setAttribute('formaction', url);
    // Restore the handle after the versioned response, including page changes.
    const observer = new win.MutationObserver(() => {
      const current = doc.getElementById(form.id) as HTMLFormElement | null;
      if (!current) { observer.disconnect(); return; }
      if (current.dataset.version === version || current.querySelector('fieldset[inert]')) return;
      const row = [...current.querySelectorAll<HTMLElement>('[data-array-item]')].find(row => row.dataset.arrayItem === rowId);
      row?.querySelector<HTMLElement>('[data-array-handle]')?.focus({ preventScroll: true });
      observer.disconnect();
      win.clearTimeout(timeout);
    });
    observer.observe(doc.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-version', 'inert'] });
    const timeout = win.setTimeout(() => observer.disconnect(), 10000);
    form.requestSubmit(button);
  }
  doc.addEventListener('click', event => {
    const button = (event.target as Element)?.closest<HTMLButtonElement>('[data-remove-array-item]');
    const row = button?.closest<HTMLElement>('[data-array-item]');
    const array = row?.closest<HTMLElement>('[data-structure-array]');
    if (!button || !row || !array || button.closest('[inert]')) return;
    event.preventDefault();
    const ids = order(array).filter(id => id !== row.dataset.arrayItem);
    const next = row.nextElementSibling?.querySelector<HTMLElement>('[data-array-handle]')
      ?? row.previousElementSibling?.querySelector<HTMLElement>('[data-array-handle]');
    row.remove(); change(array,ids,true);
    (next ?? array.querySelector<HTMLElement>('button[formaction$="/add"]'))?.focus({preventScroll:true});
  });
  doc.addEventListener('keydown', event => {
    if (event.key === 'Escape' && drag) { clear(); event.preventDefault(); return; }
    const handle = (event.target as Element)?.closest<HTMLButtonElement>('[data-array-handle]');
    if (!handle || !['ArrowUp', 'ArrowDown'].includes(event.key)) return;
    event.preventDefault();
    submit(handle, (event.key === 'ArrowUp' ? handle.dataset.up : handle.dataset.down) ?? '');
  });
  doc.addEventListener('pointerdown', event => {
    const handle = (event.target as Element)?.closest<HTMLButtonElement>('[data-array-handle]');
    const row = handle?.closest<HTMLElement>('[data-array-item]');
    const array = row?.closest<HTMLElement>('[data-array]');
    if (!handle || !row || !array || event.button !== 0 || handle.matches(':disabled') || handle.closest('[inert]')) return;
    event.preventDefault();
    handle.focus({ preventScroll: true });
    handle.setPointerCapture?.(event.pointerId);
    drag = { handle, row, array, x: event.clientX, y: event.clientY, active: false };
  });
  doc.addEventListener('pointermove', event => {
    if (!drag) return;
    if (!drag.row.isConnected || drag.handle.closest('[inert]')) { clear(); return; }
    if (Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < 5 && !drag.active) return;
    drag.active = true;
    drag.row.classList.add('array-dragging');
    drag.target?.classList.remove('array-drop-before', 'array-drop-after');
    drag.target = rows(drag.array).find(row => {
      const rect = row.getBoundingClientRect();
      return event.clientY >= rect.top && event.clientY <= rect.bottom && event.clientX >= rect.left && event.clientX <= rect.right;
    });
    if (drag.target && drag.target !== drag.row) {
      const rect = drag.target.getBoundingClientRect();
      // Chips wrap horizontally; rows and tables stack vertically.
      drag.after = drag.array.classList.contains('array-choices')
        ? event.clientX > rect.left + rect.width / 2
        : event.clientY > rect.top + rect.height / 2;
      drag.target.classList.add(drag.after ? 'array-drop-after' : 'array-drop-before');
    }
    if (event.clientY < 70) win.scrollBy(0, -18);
    else if (event.clientY > win.innerHeight - 70) win.scrollBy(0, 18);
  });
  doc.addEventListener('pointerup', () => {
    const drop = drag;
    clear();
    if (!drop?.active || !drop.target || drop.target === drop.row) return;
    const items = rows(drop.array);
    const destination = items.indexOf(drop.target) + (drop.after ? 1 : 0);
    const before = items[destination]?.dataset.arrayItem ?? drop.array.dataset.arrayNext ?? '';
    if (before === drop.row.dataset.arrayItem || items.indexOf(drop.row) + 1 === destination) return;
    submit(drop.handle, `${drop.array.dataset.array}/move?item=${encodeURIComponent(drop.row.dataset.arrayItem ?? '')}`, before);
  });
  doc.addEventListener('pointercancel', clear);
  win.addEventListener('blur', clear);
}
