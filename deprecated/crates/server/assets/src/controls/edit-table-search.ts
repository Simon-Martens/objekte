/** Enter in a table search must submit that table's action, not the edit form's first button. */
export function installEditTableSearch(document: Document) {
  document.addEventListener('keydown', event => {
    if (event.key !== 'Enter' || event.isComposing) return;
    const input = event.target;
    if (!(input instanceof document.defaultView!.HTMLInputElement) || !input.matches('.embedded-edit-search input[type="search"]')) return;
    const search = input.closest<HTMLElement>('.embedded-edit-search')?.querySelector<HTMLButtonElement>('button[formaction$="/table-search"]');
    if (!search || !input.form) return;
    event.preventDefault();
    input.form.requestSubmit(search);
  });
}
