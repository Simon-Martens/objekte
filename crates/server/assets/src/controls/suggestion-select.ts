/** Editable combobox: suggestions help entry without restricting valid free text. */
class SuggestionSelect extends HTMLElement {
  private cleanup?: AbortController;
  private observer?: MutationObserver;
  private input!: HTMLInputElement;
  private toggle!: HTMLButtonElement;
  private menu!: HTMLElement;
  private open = false;
  private browse = false;
  private selecting = false;
  private active = -1;
  private values: string[] = [];

  connectedCallback() {
    queueMicrotask(() => {
      if (!this.isConnected || this.cleanup) return;
      this.input = this.querySelector('input')!;
      this.toggle = this.querySelector('button')!;
      this.menu = this.querySelector('[role=listbox]')!;
      if (!this.input || !this.toggle || !this.menu) return;
      this.cleanup = new AbortController();
      const options = {signal: this.cleanup.signal};
      this.toggle.addEventListener('pointerdown', event => event.preventDefault(), options);
      this.toggle.addEventListener('click', () => {
        this.input.focus({preventScroll: true});
        if (this.menu.matches(':popover-open')) this.close();
        else this.show(true);
      }, options);
      this.input.addEventListener('input', () => { if (!this.selecting) this.show(false); }, options);
      this.input.addEventListener('keydown', event => {
        if (event.key === 'Escape' && this.open) {
          event.preventDefault(); event.stopPropagation(); this.close();
        } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          event.preventDefault();
          if (!this.menu.matches(':popover-open')) this.show(true);
          this.active = this.values.length ? Math.max(0, Math.min(this.values.length - 1, this.active + (event.key === 'ArrowDown' ? 1 : -1))) : -1;
          this.highlight();
        } else if (event.key === 'Enter' && this.open && this.active >= 0) {
          event.preventDefault(); this.choose(this.active);
        } else if (event.key === 'Tab') this.close();
      }, options);
      this.menu.addEventListener('pointerdown', event => event.preventDefault(), options);
      this.menu.addEventListener('click', event => {
        const option = (event.target as Element).closest<HTMLElement>('[data-option]');
        if (option) this.choose(Number(option.dataset.option));
      }, options);
      document.addEventListener('pointerdown', event => { if (!this.contains(event.target as Node)) this.close(); }, options);
      document.addEventListener('focusin', event => { if (!this.contains(event.target as Node)) this.close(); }, options);
      document.addEventListener('submit', () => this.close(), options);
      document.addEventListener('scroll', () => this.position(), {...options, capture: true});
      window.addEventListener('resize', () => this.position(), options);
      window.addEventListener('navigate', () => this.close(), options);
      this.observer = new MutationObserver(records => {
        if (records.some(record => (record.target as Element).closest?.('datalist') || [...record.addedNodes].some(node => node instanceof HTMLDataListElement))) {
          if (this.open) this.render();
        }
      });
      this.observer.observe(this, {childList: true, subtree: true, attributes: true, attributeFilter: ['value', 'data-query']});
    });
  }
  private show(browse: boolean) {
    this.browse = browse;
    this.open = true;
    this.active = -1;
    this.render();
    if (browse) this.input.dispatchEvent(new CustomEvent('browse-suggestions'));
  }
  private render() {
    const source = this.querySelector('datalist');
    const query = this.input.value;
    const fresh = source && (source.dataset.query === '' || (!this.browse && source.dataset.query === query));
    this.values = fresh ? [...source.options].map(option => option.value).filter(value => this.browse || (value !== query && value.toLocaleLowerCase().includes(query.toLocaleLowerCase()))) : [];
    this.active = -1;
    this.input.removeAttribute('aria-activedescendant');
    this.menu.replaceChildren();
    for (const [index, value] of this.values.entries()) {
      const option = document.createElement('div');
      option.id = `${this.menu.id}-${index}`;
      option.dataset.option = String(index);
      option.setAttribute('role', 'option');
      option.setAttribute('aria-selected', String(value === query));
      option.textContent = value;
      this.menu.append(option);
    }
    // An empty list can mean a pending lookup, a stale response or no matches.
    // Keep listening for results without flashing an empty-state popup.
    const visible = this.values.length > 0;
    this.input.setAttribute('aria-expanded', String(visible));
    this.toggle.setAttribute('aria-expanded', String(visible));
    if (visible) {
      if (!this.menu.matches(':popover-open')) this.menu.showPopover();
      this.position();
    } else {
      this.menu.hidePopover();
    }
  }
  private highlight() {
    this.menu.querySelectorAll<HTMLElement>('[data-option]').forEach((option, index) => {
      option.classList.toggle('active', index === this.active);
      if (index === this.active) {
        this.input.setAttribute('aria-activedescendant', option.id);
        option.scrollIntoView({block: 'nearest'});
      }
    });
  }
  private choose(index: number) {
    if (this.values[index] === undefined) return;
    this.input.value = this.values[index];
    this.selecting = true;
    this.input.dispatchEvent(new Event('input', {bubbles: true}));
    this.input.dispatchEvent(new Event('change', {bubbles: true}));
    this.selecting = false;
    this.close();
    this.input.focus({preventScroll: true});
  }
  private position() {
    if (!this.open || !this.menu.matches(':popover-open')) return;
    const rect = this.getBoundingClientRect();
    const below = window.innerHeight - rect.bottom - 8;
    const above = rect.top - 8;
    const up = below < 160 && above > below;
    this.menu.style.width = `${Math.min(rect.width, window.innerWidth - 16)}px`;
    this.menu.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - this.menu.offsetWidth - 8))}px`;
    this.menu.style.maxHeight = `${Math.max(40, Math.min(260, up ? above : below))}px`;
    this.menu.style.top = `${up ? Math.max(8, rect.top - this.menu.offsetHeight - 4) : rect.bottom + 4}px`;
  }
  private close() {
    if (!this.open) return;
    this.open = false;
    this.menu.hidePopover();
    this.input.setAttribute('aria-expanded', 'false');
    this.toggle.setAttribute('aria-expanded', 'false');
    this.input.removeAttribute('aria-activedescendant');
    this.active = -1;
  }
  disconnectedCallback() {
    this.close();
    this.cleanup?.abort();
    this.cleanup = undefined;
    this.observer?.disconnect();
  }
}
customElements.define('suggestion-select', SuggestionSelect);
