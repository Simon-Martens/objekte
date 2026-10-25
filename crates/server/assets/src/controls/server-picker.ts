/** Focus and keyboard mechanics only. Every choice and close command goes to Rust. */
class ServerPicker extends HTMLElement {
  private previous?: HTMLElement;
  private ready?: MutationObserver;
  private backgroundObserver?: MutationObserver;
  private background: HTMLElement[] = [];
  private resize?: ResizeObserver;
  private dropdown = false;
  connectedCallback() {
    this.dropdown = this.hasAttribute('data-popover');
    this.addEventListener('keydown', this.keydown);
    this.addEventListener('click', this.backdrop);
    queueMicrotask(() => {
      if (!this.isConnected) return;
      this.previous = document.activeElement instanceof HTMLElement ? document.activeElement : undefined;
      const syncBackground = () => {
        this.background = [...document.querySelectorAll<HTMLElement>('.edit-grid, .edit-heading, .record-heading, .record-preview, .child-list, .parent-links, .references-panel, #project-navigation, #admin-header')];
        this.background.forEach(element => { if(!element.inert) element.inert = true; });
      };
      if (this.dropdown) {
        this.position();
        window.addEventListener('resize', this.position);
        window.addEventListener('scroll', this.position, true);
        document.addEventListener('pointerdown', this.outside);
        this.resize = new ResizeObserver(this.position); this.resize.observe(this);
      } else {
      syncBackground();
      this.backgroundObserver=new MutationObserver(syncBackground);
      this.backgroundObserver.observe(document.body,{childList:true,subtree:true,attributes:true,attributeFilter:['inert']});
      }
      const focus = () => {
        if (this.closest('fieldset[inert]')) return;
        const target = this.querySelector<HTMLElement>('input[name="picker_query"], input[type="search"]')
          ?? this.querySelector<HTMLElement>('[data-close-picker], [role="dialog"]');
        target?.focus({preventScroll: true});
        this.ready?.disconnect();
      };
      this.ready = new MutationObserver(focus);
      const fieldset = this.closest('fieldset');
      if (fieldset) this.ready.observe(fieldset, { attributes: true, attributeFilter: ['inert'] });
      focus();
    });
  }
  private position = () => {
    const anchor = document.getElementById(`field-${this.dataset.returnField}`)?.querySelector('[data-select-anchor]');
    if (!anchor || !this.isConnected) return;
    const box = anchor.getBoundingClientRect();
    const width = Math.min(Math.max(box.width, 300), window.innerWidth - 24);
    this.style.width = `${width}px`;
    this.style.left = `${Math.max(12, Math.min(box.left, window.innerWidth - width - 12))}px`;
    const below = window.innerHeight - box.bottom - 12, above = box.top - 12;
    const flip = below < 240 && above > below;
    this.style.maxHeight = `${Math.max(120, Math.min(420, flip ? above : below))}px`;
    this.style.top = flip ? 'auto' : `${Math.max(12,box.bottom + 4)}px`;
    this.style.bottom = flip ? `${window.innerHeight - box.top + 4}px` : 'auto';
  };
  private outside = (event: PointerEvent) => {
    if (!this.contains(event.target as Node) && !this.closest('[inert]')) this.querySelector<HTMLElement>('[data-close-picker]')?.click();
  };
  private backdrop = (event: MouseEvent) => {
    if (event.target === this && !this.closest('fieldset[inert]')) {
      this.querySelector<HTMLElement>('[data-close-picker]')?.click();
    }
  };
  private keydown = (event: KeyboardEvent) => {
    if (event.key === 'Enter' && (event.target as HTMLInputElement).name === 'picker_query') {
      event.preventDefault();
      this.querySelector<HTMLButtonElement>('[data-picker-search]')?.click();
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      this.querySelector<HTMLElement>('[data-close-picker]')?.click();
    }
    if (event.key !== 'Tab' || this.dropdown) return;
    const controls = [...this.querySelectorAll<HTMLElement>('button:not(:disabled), input:not([type="hidden"]):not(:disabled), select:not(:disabled), a[href], [tabindex="0"]')].filter(element => element.getClientRects().length);
    const first = controls[0], last = controls.at(-1);
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
  };
  disconnectedCallback() {
    queueMicrotask(() => {
      if (this.isConnected) return;
      this.ready?.disconnect();
      this.resize?.disconnect();
      window.removeEventListener('resize', this.position);
      window.removeEventListener('scroll', this.position, true);
      document.removeEventListener('pointerdown', this.outside);
      this.backgroundObserver?.disconnect();
      this.removeEventListener('keydown', this.keydown);
      this.removeEventListener('click', this.backdrop);
      if(document.querySelector('server-picker')) return;
      this.background.forEach(element => { element.inert = false; });
      const field = document.getElementById(`field-${this.dataset.returnField}`);
      const control = field?.querySelector<HTMLElement>('[data-reference-picker], code-input, input:not([type="checkbox"]):not([type="hidden"]), select, textarea');
      const target = control ?? (this.dataset.returnSelector
        ? document.querySelector<HTMLElement>(this.dataset.returnSelector) : this.previous);
      if (!target?.isConnected) return;
      // The response can remove the picker before Datastar clears its pending
      // fieldset. Restoring focus while that ancestor is inert has no effect.
      const pending = target.closest('fieldset[inert]');
      if (!pending) { target.focus(); return; }
      const restore = new MutationObserver(() => {
        if (!target.isConnected || !pending.hasAttribute('inert')) {
          restore.disconnect();
          if (target.isConnected) target.focus();
        }
      });
      restore.observe(pending, { attributes: true, attributeFilter: ['inert'] });
    });
  }
}
customElements.define('server-picker', ServerPicker);

// Collection changes browse immediately; search remains scoped to that collection.
document.addEventListener('change', event => {
  const select = event.target;
  if (!(select instanceof HTMLSelectElement) || !select.matches('[data-picker-collection]')) return;
  select.closest('server-picker')?.querySelector<HTMLButtonElement>('[data-picker-search]')?.click();
});

// A pending form is inert, which clears browser focus. Restore it after each
// picker update so keyboard users can continue selecting without reopening it.
document.addEventListener('submit', event => {
  const button = (event as SubmitEvent).submitter;
  const picker = button?.closest<HTMLElement>('server-picker');
  const form = event.target;
  if (!picker || !(form instanceof HTMLFormElement) || !(button instanceof HTMLButtonElement)) return;
  const action = button.getAttribute('formaction') ?? '';
  if (!['toggle-choice', 'search-picker', 'next-picker', 'previous-picker'].some(name => action.endsWith('/' + name))) return;
  const field = picker.dataset.returnField;
  const choice = button.name === 'choice' ? button.value : null;
  const version = form.dataset.version;
  const scroll = picker.querySelector('.picker-results')?.scrollTop ?? 0;
  const observer = new MutationObserver(() => {
    const current = document.querySelector<HTMLElement>('server-picker');
    const currentForm = current?.closest('form');
    if (!current || current.dataset.returnField !== field) { observer.disconnect(); clearTimeout(timeout); return; }
    if (currentForm?.dataset.version === version || current.closest('fieldset[inert]')) return;
    const target = choice === null ? null : [...current.querySelectorAll<HTMLButtonElement>('button[name="choice"]')].find(button => button.value === choice);
    (target ?? current.querySelector<HTMLElement>('input[name="picker_query"]'))?.focus({preventScroll:true});
    const results = current.querySelector('.picker-results');
    if (results) results.scrollTop = choice === null ? 0 : scroll;
    observer.disconnect(); clearTimeout(timeout);
  });
  observer.observe(document.body, { childList:true, subtree:true, attributes:true, attributeFilter:['data-version','inert'] });
  const timeout = setTimeout(() => observer.disconnect(), 10000);
}, true);
