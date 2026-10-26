/** Project changes reload the document; draft storage is scoped to the project. */
export function installProjectChooser(document: Document, leave: () => Promise<boolean>, submit: (form: HTMLFormElement) => void) {
  document.addEventListener('change', async event => {
    const select = (event.target as Element)?.closest<HTMLSelectElement>('#project-select');
    const form = select?.closest<HTMLFormElement>('form[data-project-chooser]');
    if (!select || !form || select.value === form.dataset.projectId) return;
    const next = select.value;
    select.value = form.dataset.projectId ?? '';
    if (!await leave() || !form.isConnected) return;
    select.value = next;
    submit(form);
  });
}

export function projectDraftKey(project: string, url: string) { return `${project}:${url}`; }
