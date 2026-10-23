// Keep widths across list patches (sorting, filtering and pagination).
const widths = new Map<string, number[]>();
const desktop = window.matchMedia('(min-width: 721px)');

function enhanceTables() {
  document.querySelectorAll<HTMLTableElement>('.data-table').forEach((table, tableIndex) => {
    const headers = Array.from(table.tHead?.rows[0]?.cells ?? []);
    if (!headers.length) return;
    const path = table.closest<HTMLElement>('[data-url]')?.dataset.url?.split('?')[0] ?? location.pathname;
    const key = JSON.stringify([path, tableIndex, headers.map(header => header.querySelector('a')?.textContent ?? header.textContent)]);
    const apply = (sizes: number[]) => {
      table.classList.add('columns-resized');
      table.style.setProperty('--table-width', `${sizes.reduce((sum, size) => sum + size, 0)}px`);
      headers.forEach((header, index) => header.style.setProperty('--column-width', `${sizes[index]}px`));
    };
    const saved = widths.get(key);
    if (saved?.length === headers.length) apply(saved);
    headers.forEach((header, index) => {
      if (!header.querySelector('a') || header.querySelector('.column-resize-handle')) return;
      const handle = document.createElement('span');
      handle.className = 'column-resize-handle';
      handle.tabIndex = 0;
      handle.setAttribute('role', 'separator');
      handle.setAttribute('aria-orientation', 'vertical');
      handle.setAttribute('aria-label', `Resize ${header.textContent?.trim()} column`);
      handle.setAttribute('aria-valuemin', '96');
      const resize = (sizes: number[], value: number) => {
        sizes[index] = Math.max(96, Math.round(value));
        widths.set(key, sizes);
        apply(sizes);
        handle.setAttribute('aria-valuenow', String(sizes[index]));
      };
      handle.addEventListener('click', event => { event.preventDefault(); event.stopPropagation(); });
      handle.addEventListener('pointerdown', event => {
        if (event.button !== 0 || !desktop.matches) return;
        event.preventDefault();
        event.stopPropagation();
        const sizes = headers.map(cell => cell.getBoundingClientRect().width);
        const start = event.clientX;
        const initial = sizes[index];
        handle.setPointerCapture(event.pointerId);
        document.documentElement.classList.add('resizing-column');
        const move = (next: PointerEvent) => resize(sizes, initial + next.clientX - start);
        const finish = () => {
          document.documentElement.classList.remove('resizing-column');
          handle.removeEventListener('pointermove', move);
          handle.removeEventListener('lostpointercapture', finish);
        };
        handle.addEventListener('pointermove', move);
        handle.addEventListener('lostpointercapture', finish);
      });
      handle.addEventListener('keydown', event => {
        if (!desktop.matches || !['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
        event.preventDefault();
        const sizes = headers.map(cell => cell.getBoundingClientRect().width);
        resize(sizes, sizes[index] + (event.key === 'ArrowRight' ? 16 : -16));
      });
      header.append(handle);
    });
  });
}
new MutationObserver(enhanceTables).observe(document.body, {childList: true, subtree: true});
enhanceTables();
