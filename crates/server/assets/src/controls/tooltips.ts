import tippy, { hideAll, type Instance } from 'tippy.js';
import 'tippy.js/dist/tippy.css';

// Rust owns labels; Tippy/Popper owns focus, hover and viewport positioning.
const selector = '[data-tooltip], .icon-button[aria-label], .account-link[aria-label]';
const instances = new Map<HTMLElement, Instance>();
const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');

function sync(element: HTMLElement) {
  const content = element.dataset.tooltip ?? element.getAttribute('aria-label') ?? '';
  const existing = instances.get(element);
  if (!element.matches(selector) || !content.trim()) {
    existing?.destroy();
    instances.delete(element);
    return;
  }
  if (existing) {
    if (existing.props.content !== content) existing.setContent(content);
    return;
  }
  instances.set(element, tippy(element, {
    content,
    allowHTML: false,
    ignoreAttributes: true,
    theme: 'editor',
    arrow: false,
    placement: 'top',
    delay: [350, 100],
    duration: reducedMotion.matches ? 0 : [120, 80],
    maxWidth: 320,
    offset: [0, 8],
    // Hoverable text, without treating the tooltip as a menu or popover.
    interactive: true,
    aria: { content: 'describedby', expanded: false },
    appendTo: reference => reference.closest('dialog, [popover]') ?? document.body,
    touch: false,
    zIndex: 100,
    popperOptions: {
      strategy: 'fixed',
      modifiers: [{ name: 'preventOverflow', options: { padding: 8 } }],
    },
    onShow(instance) {
      if (element.closest('[inert]') || !element.isConnected) return false;
      hideAll({ exclude: instance, duration: 0 });
    },
  }));
}

function scan(root: Element) {
  if (root instanceof HTMLElement && root.matches(selector)) sync(root);
  root.querySelectorAll<HTMLElement>(selector).forEach(sync);
}

// Fragments can add, replace or relabel controls without reloading the shell.
new MutationObserver(records => {
  for (const [element, instance] of instances) {
    if (!element.isConnected) {
      instance.destroy();
      instances.delete(element);
    }
  }
  for (const record of records) {
    if (record.type === 'attributes' && record.target instanceof HTMLElement) sync(record.target);
    record.addedNodes.forEach(node => { if (node instanceof Element) scan(node); });
  }
}).observe(document.body, {
  childList: true, subtree: true, attributes: true,
  attributeFilter: ['data-tooltip', 'aria-label'],
});
scan(document.body);

const dismiss = () => {
  instances.forEach(instance => instance.clearDelayTimeouts());
  hideAll({ duration: 0 });
};
document.addEventListener('keydown', event => { if (event.key === 'Escape') dismiss(); });
document.addEventListener('pointerdown', dismiss);
document.addEventListener('submit', dismiss, true);
window.addEventListener('navigate', dismiss);
window.addEventListener('blur', dismiss);
reducedMotion.addEventListener('change', () => {
  instances.forEach(instance => instance.setProps({ duration: reducedMotion.matches ? 0 : [120, 80] }));
});
