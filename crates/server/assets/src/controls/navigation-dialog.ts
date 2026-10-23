/** The shell owns this confirmation; it does not change any project data. */
export function confirmNavigation(): Promise<boolean> {
  const dialog = document.querySelector<HTMLDialogElement>('#navigation-dialog');
  if (!dialog) return Promise.resolve(confirm('Leave this view with unsaved changes?'));
  if (dialog.open) return Promise.resolve(false);
  return new Promise(resolve => {
    dialog.returnValue = 'stay';
    dialog.addEventListener('close', () => resolve(dialog.returnValue === 'leave'), { once: true });
    dialog.showModal();
  });
}
