/** The shell owns this confirmation; it does not change any project data. */
export function confirmNavigation(): Promise<boolean> {
  const dialog = document.querySelector<HTMLDialogElement>('#navigation-dialog');
  if (!dialog) return Promise.resolve(confirm('Leave this view with unsaved changes?'));
  if (dialog.open) return Promise.resolve(false);
  return new Promise(resolve => {
    dialog.returnValue = 'stay';
    const message = dialog.querySelector('p');
    if (message) message.textContent = document.querySelector('#record-form')
      ? 'Your changes are not saved. You can return to this draft in this tab until it expires.'
      : 'Your changes are not saved. Leaving this view will discard them.';
    dialog.addEventListener('close', () => resolve(dialog.returnValue === 'leave'), { once: true });
    dialog.showModal();
  });
}
