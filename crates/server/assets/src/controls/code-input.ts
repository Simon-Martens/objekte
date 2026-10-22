import type { editor, IDisposable } from 'monaco-editor/editor/editor.api.js';

/** Owns Monaco's internal DOM only; Rust supplies authoritative source and versions. */
class CodeInput extends HTMLElement {
  static observedAttributes = ['data-version'];
  private view?: editor.IStandaloneCodeEditor;
  private model?: editor.ITextModel;
  private subscriptions: IDisposable[] = [];
  private resize?: ResizeObserver;
  private mounting = false;
  private appliedVersion = '';
  private syncing = false;

  async connectedCallback() {
    if (this.view || this.mounting) return;
    this.mounting = true;
    try {
      const { monaco } = await import('./monaco');
      if (!this.isConnected) return;
      const input = this.querySelector<HTMLTextAreaElement>('textarea')!;
      const host = this.querySelector<HTMLElement>('.code-host')!;
      const inline = this.hasAttribute('data-inline');
      this.model = monaco.editor.createModel(input.value, this.dataset.language);
      this.model.updateOptions({ tabSize: 2, insertSpaces: true });
      this.view = monaco.editor.create(host, {
        model: this.model, theme: `objekte-${document.documentElement.dataset.theme || 'light'}`,
        ariaLabel: this.dataset.label || 'Source editor',
        readOnly: this.isReadOnly(), domReadOnly: this.isReadOnly(),
        fontFamily: '"JetBrains Mono", monospace',
        fontSize: Number(document.getElementById('ui-preferences')?.dataset.editorFont || 13), lineHeight: 22,
        minimap: { enabled: false }, wordWrap: 'on',
        scrollBeyondLastLine: false, overviewRulerLanes: 0,
        hideCursorInOverviewRuler: true, glyphMargin: false,
        lineNumbers: 'on', lineNumbersMinChars: 3,
        lineDecorationsWidth: inline ? 12 : 8,
        folding: false, renderLineHighlight: 'none',
        padding: { top: 12, bottom: 12 },
        scrollbar: { verticalScrollbarSize: 8, horizontalScrollbarSize: 8, alwaysConsumeMouseWheel: false },
        stickyScroll: { enabled: false },
        quickSuggestions: this.dataset.language === 'html',
        fixedOverflowWidgets: true,
        // Tab moves between form fields by default; Ctrl+M enables code indentation.
        tabFocusMode: true,
        links: false,
      });
      const layout = () => {
        if (!this.view) return;
        const height = Math.min(inline ? 180 : 480, Math.max(inline ? 46 : this.dataset.language === 'html' ? 200 : 100, this.view.getContentHeight()));
        host.style.height = `${height}px`;
        this.view.layout({ width: host.clientWidth, height });
      };
      this.subscriptions.push(
        this.view.onDidContentSizeChange(layout),
        this.view.onDidChangeCursorSelection(() => this.syncSelection()),
        this.view.onDidChangeModelContent(() => {
          if (this.syncing) return;
          input.value = this.model!.getValue();
          this.syncSelection();
          input.dispatchEvent(new Event('input', { bubbles: true }));
        }),
      );
      if (this.dataset.language === 'markdown') {
        this.subscriptions.push(this.view.addAction({
          id: 'objekte.insert-link', label: 'Insert or edit link',
          keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyK],
          run: () => {
            if (this.isReadOnly()) return;
            this.closest('.source-control')?.querySelector<HTMLButtonElement>('[data-insert-link]')?.click();
          },
        }));
      }
      this.resize = new ResizeObserver(layout);
      this.resize.observe(host);
      input.hidden = true;
      this.appliedVersion = this.dataset.version ?? '';
      layout();
    } catch (error) {
      // The ordinary textarea remains usable if the optional editor cannot load.
      this.dispose();
      console.error('Could not load the source editor.', error);
    } finally {
      this.mounting = false;
    }
  }

  focus(options?: FocusOptions) {
    if (this.view) this.view.focus();
    else this.querySelector<HTMLTextAreaElement>('textarea')?.focus(options);
  }

  private isReadOnly() { return !!this.closest('fieldset[disabled]'); }

  private syncSelection() {
    const selection = this.view?.getSelection();
    if (!selection || !this.model) return;
    const input = this.querySelector<HTMLTextAreaElement>('textarea')!;
    const suffix = input.name.slice('value.'.length);
    // Monaco and the core link command both use UTF-16 offsets.
    for (const [key, position] of [
      ['selection_from', selection.getStartPosition()],
      ['selection_to', selection.getEndPosition()],
    ] as const) {
      const field = this.closest('form')?.elements.namedItem(`${key}.${suffix}`);
      if (field instanceof HTMLInputElement) field.value = String(this.model.getOffsetAt(position));
    }
  }

  attributeChangedCallback() { queueMicrotask(() => this.applyServerValue()); }

  private applyServerValue() {
    if (!this.view || !this.model || this.appliedVersion === this.dataset.version) return;
    const input = this.querySelector<HTMLTextAreaElement>('textarea')!;
    // A morphed textarea's value can be dirty; textContent contains the server value.
    const value = input.textContent ?? '';
    if (value !== this.model.getValue()) {
      this.syncing = true;
      const selection = this.view.getSelection();
      this.view.pushUndoStop();
      this.view.executeEdits('server', [{ range: this.model.getFullModelRange(), text: value }]);
      this.view.pushUndoStop();
      if (selection) this.view.setSelection(selection);
      this.syncing = false;
    }
    this.view.updateOptions({ readOnly: this.isReadOnly(), domReadOnly: this.isReadOnly(), ariaLabel: this.dataset.label || 'Source editor' });
    input.value = value;
    input.hidden = true;
    this.syncSelection();
    this.appliedVersion = this.dataset.version ?? '';
  }

  private dispose() {
    this.resize?.disconnect();
    this.subscriptions.splice(0).forEach(subscription => subscription.dispose());
    this.view?.dispose();
    this.model?.dispose();
    this.view = undefined;
    this.model = undefined;
  }

  disconnectedCallback() {
    // A morph may move the same element; dispose only when it actually leaves the page.
    queueMicrotask(() => { if (!this.isConnected) this.dispose(); });
  }
}
customElements.define('code-input', CodeInput);
