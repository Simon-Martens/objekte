/** One out-of-flow feedback surface for server commands and connection errors. */
function syncNotifications() {
  const region = document.getElementById('notification-region');
  if (!region) return;
  const notices = [...document.querySelectorAll<HTMLTemplateElement>('template[data-notification]')];
  const messages = notices.map(notice => ({ kind: notice.dataset.notification ?? 'success', text: notice.content.textContent?.trim() ?? '' })).filter(notice => notice.text);
  notices.forEach(notice => notice.remove());
  if (messages.length) {
    const kind = messages.some(n => n.kind === 'error') ? 'error' : messages.some(n => n.kind === 'warning') ? 'warning' : 'success';
    region.dataset.kind = kind;
    region.setAttribute('role', kind === 'error' ? 'alert' : 'status');
    region.setAttribute('aria-live', kind === 'error' ? 'assertive' : 'polite');
    region.textContent = messages.map(n => n.text).join(' ');
  }
  if (!region.textContent?.trim() || region.querySelector('.notification-icon')) return;
  const kind = region.getAttribute('role') === 'alert' ? 'error' : region.dataset.kind ?? 'success';
  region.dataset.kind = kind;
  const text = document.createElement('span');
  text.className = 'notification-message';
  text.textContent = region.textContent;
  const icon = document.createElement('i');
  icon.className = `notification-icon ${kind === 'error' ? 'ri-error-warning-line' : kind === 'warning' ? 'ri-alert-line' : 'ri-checkbox-circle-line'}`;
  icon.setAttribute('aria-hidden', 'true');
  const dismiss = document.createElement('button');
  dismiss.type = 'button';
  dismiss.className = 'icon-button small notification-dismiss';
  dismiss.setAttribute('aria-label', 'Dismiss notification');
  dismiss.innerHTML = '<i class="ri-close-line" aria-hidden="true"></i>';
  dismiss.addEventListener('click', () => region.replaceChildren());
  region.replaceChildren(icon, text, dismiss);
}
new MutationObserver(syncNotifications).observe(document.body, {childList:true, subtree:true});
syncNotifications();
