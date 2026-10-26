/** Connection feedback for requests that cannot reach Rust's error renderer. */
document.addEventListener('datastar-fetch', event => {
  const { type } = (event as CustomEvent<{ type: string }>).detail;
  const notice = document.getElementById('notification-region');
  if (!notice) return;
  if (type === 'started' && notice.dataset.networkError) {
    notice.textContent = '';
    notice.removeAttribute('data-network-error');
    notice.setAttribute('role', 'status');
    notice.setAttribute('aria-live', 'polite');
  }
  if (type !== 'retrying' && type !== 'retries-failed') return;
  notice.dataset.networkError = 'true';
  notice.dataset.kind = type === 'retrying' ? 'warning' : 'error';
  notice.setAttribute('role', type === 'retrying' ? 'status' : 'alert');
  notice.setAttribute('aria-live', 'assertive');
  notice.textContent = type === 'retrying'
    ? 'Connection interrupted. Waiting to reconnect…'
    : 'Connection interrupted. The request could not be confirmed. Keep this view open and check the record before retrying.';
});
