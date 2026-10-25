/** Choice chips are ordinary unsaved form input. No network request is made here. */
export function installChoiceValues(win: Window & typeof globalThis) {
  const doc = win.document;
  const chips = (field: HTMLElement) => [...field.querySelectorAll<HTMLElement>(':scope > [data-choice-value]')];
  const values = (field: HTMLElement) => chips(field).map(chip => JSON.parse(chip.dataset.choiceValue!));
  const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
  const syncPicker = () => {
    const picker = doc.querySelector<HTMLElement>('server-picker[data-popover]');
    const field = doc.getElementById(`field-${picker?.dataset.returnField}`)?.querySelector<HTMLElement>('[data-local-choices]');
    if (!picker || !field) return;
    const selected = values(field);
    for (const button of picker.querySelectorAll<HTMLButtonElement>('button[name=choice]')) {
      const checked = selected.some(value => same(value, JSON.parse(button.value)));
      button.setAttribute('aria-checked', String(checked));
      const icon = button.querySelector('i');
      if (icon) icon.className = checked ? 'ri-checkbox-line' : 'ri-checkbox-blank-line';
    }
    const count = picker.querySelector<HTMLElement>('[data-selection-count]');
    const label = count?.dataset.selectionCount?.replace('{count}', String(selected.length));
    if (count && label && count.textContent !== label) count.textContent = label;
  };
  const change = (field: HTMLElement) => {
    let hidden = field.querySelector<HTMLInputElement>('[data-choice-values]');
    if (!hidden) { hidden = doc.createElement('input'); hidden.type = 'hidden'; hidden.dataset.choiceValues = ''; hidden.name = `choices.${field.dataset.localChoices}`; field.append(hidden); }
    hidden.value = JSON.stringify(values(field));
    const items = chips(field);
    items.forEach((chip, index) => {
      const handle = chip.querySelector<HTMLButtonElement>('[data-array-handle]');
      if (!handle) return;
      handle.dataset.up = index ? `${field.dataset.array}/up?item=${chip.dataset.arrayItem}` : '';
      handle.dataset.down = index < items.length - 1 ? `${field.dataset.array}/down?item=${chip.dataset.arrayItem}` : '';
      handle.setAttribute('aria-label', (field.dataset.reorderLabel ?? 'Reorder item {number}').replace('{number}', String(index + 1)));
    });
    field.querySelector('[data-choice-limit]')?.remove();
    syncPicker();
    // Existing dirty tracking, refresh recovery and read-only validation consume this.
    field.dispatchEvent(new win.Event('input', {bubbles:true}));
    const undo = doc.querySelector<HTMLButtonElement>('[data-history=undo]');
    if (undo) undo.disabled = false;
  };
  const add = (field: HTMLElement, value: unknown, label: string) => {
    if (values(field).some(existing => same(existing, value))) return true;
    if (field.dataset.maxItems && chips(field).length >= Number(field.dataset.maxItems)) {
      let notice = field.querySelector<HTMLElement>('[data-choice-limit]');
      if (!notice) { notice = doc.createElement('p'); notice.dataset.choiceLimit = ''; notice.className = 'field-errors'; notice.setAttribute('role','alert'); field.append(notice); }
      notice.textContent = field.dataset.limitMessage!; return false;
    }
    const chip = doc.createElement('div'); chip.className = 'array-choice';
    chip.dataset.arrayItem = `local-${win.crypto.randomUUID()}`; chip.dataset.choiceValue = JSON.stringify(value);
    const handle = doc.createElement('button'); handle.type = 'button'; handle.className = 'icon-button array-handle'; handle.dataset.arrayHandle = '';
    handle.innerHTML = '<i class="ri-draggable" aria-hidden="true"></i>';
    const text = doc.createElement('span'); text.className = 'array-choice-label'; text.textContent = label;
    const remove = doc.createElement('button'); remove.type = 'button'; remove.className = 'icon-button small'; remove.dataset.removeChoice = '';
    remove.setAttribute('aria-label',(field.dataset.removeLabel ?? 'Remove {name}').replace('{name}',label));
    remove.innerHTML = '<i class="ri-close-line" aria-hidden="true"></i>';
    chip.append(handle,text,remove); field.insertBefore(chip,field.querySelector('[data-choice-input]')); change(field); return true;
  };
  doc.addEventListener('click', event => {
    const button = (event.target as Element)?.closest<HTMLButtonElement>('button');
    if (!button) return;
    const field = button.closest<HTMLElement>('[data-local-choices]');
    if (field && button.hasAttribute('data-remove-choice')) {
      event.preventDefault();
      button.closest('[data-choice-value]')?.remove(); change(field);
      field.querySelector<HTMLInputElement>('[data-choice-input]')?.focus({preventScroll:true});
    }
    const picker = button.closest<HTMLElement>('server-picker[data-popover]');
    if (picker && button.name === 'choice') {
      const target = doc.getElementById(`field-${picker.dataset.returnField}`)?.querySelector<HTMLElement>('[data-local-choices]');
      if (!target) return;
      event.preventDefault();
      const value = JSON.parse(button.value), selected = chips(target).filter(chip => same(JSON.parse(chip.dataset.choiceValue!), value));
      if (selected.length) { selected.forEach(chip => chip.remove()); change(target); }
      else add(target,value,button.querySelector('span')?.textContent ?? button.textContent ?? '');
    }
  });
  doc.addEventListener('choice-move', event => {
    const field = event.target as HTMLElement;
    if (!field.matches('[data-local-choices]')) return;
    const {item, before} = (event as CustomEvent<{item:string;before:string}>).detail;
    const items = chips(field), chip = items.find(chip => chip.dataset.arrayItem === item);
    if (!chip) return;
    field.insertBefore(chip, items.find(chip => chip.dataset.arrayItem === before) ?? field.querySelector('[data-choice-input]'));
    change(field); chip.querySelector<HTMLElement>('[data-array-handle]')?.focus({preventScroll:true});
  });
  // A read-only Browse search can return server selections older than local input.
  new win.MutationObserver(syncPicker).observe(doc.body,{childList:true,subtree:true});
  return {values, add};
}
