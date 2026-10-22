/** Focus and keyboard mechanics only. Every choice and close command goes to Rust. */
class ServerPicker extends HTMLElement {
  private previous?: HTMLElement;
  private ready?: MutationObserver;
  private backgroundObserver?: MutationObserver;
  private background: HTMLElement[] = [];
  connectedCallback() {
    this.addEventListener('keydown', this.keydown);
    this.addEventListener('click', this.backdrop);
    queueMicrotask(() => {
      if (!this.isConnected) return;
      this.previous = document.activeElement instanceof HTMLElement ? document.activeElement : undefined;
      const syncBackground = () => {
        this.background = [...document.querySelectorAll<HTMLElement>('.edit-grid, .edit-heading, .record-heading, .record-preview, .child-list, .parent-links, .references-panel, #project-navigation, #admin-header')];
        this.background.forEach(element => { if(!element.inert) element.inert = true; });
      };
      syncBackground();
      this.backgroundObserver=new MutationObserver(syncBackground);
      this.backgroundObserver.observe(document.body,{childList:true,subtree:true,attributes:true,attributeFilter:['inert']});
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
    if (event.key !== 'Tab') return;
    const controls = [...this.querySelectorAll<HTMLElement>('button:not(:disabled), input:not([type="hidden"]):not(:disabled), select:not(:disabled), a[href], [tabindex="0"]')].filter(element => element.getClientRects().length);
    const first = controls[0], last = controls.at(-1);
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
  };
  disconnectedCallback() {
    queueMicrotask(() => {
      if (this.isConnected) return;
      this.ready?.disconnect();
      this.backgroundObserver?.disconnect();
      this.removeEventListener('keydown', this.keydown);
      this.removeEventListener('click', this.backdrop);
      if(document.querySelector('server-picker')) return;
      this.background.forEach(element => { element.inert = false; });
      const field = document.getElementById(`field-${this.dataset.returnField}`);
      const control = field?.querySelector<HTMLElement>('code-input, input:not([type="checkbox"]):not([type="hidden"]), select, textarea');
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
