// Expansion is local interaction state; node contents and destinations come from Rust.
const expanded = new Map<string, boolean>();
document.addEventListener('toggle', event => {
  const node = event.target;
  if (node instanceof HTMLDetailsElement && node.id && node.closest('#project-navigation')) expanded.set(node.id,node.open);
}, true);
// Label navigation opens the containing branch. The chevron remains a native
// summary toggle, so collapsing never changes the selected record.
window.addEventListener('navigate', ((event: CustomEvent<{url:string}>) => {
  for (const link of document.querySelectorAll<HTMLAnchorElement>('#project-navigation a[data-nav]')) {
    if(link.pathname+link.search!==event.detail.url) continue;
    let node: Element|null=link;
    while(node) {
      if(node instanceof HTMLDetailsElement){node.open=true;expanded.set(node.id,true);}
      node=node.parentElement;
    }
  }
}) as EventListener);
let activeWorkspace='';
new MutationObserver(() => {
  const next = document.getElementById('project-navigation');
  if (!next) return;
  const workspace=document.getElementById('workspace');
  const active=workspace?.dataset.recordUrl || workspace?.dataset.url || '';
  if(active!==activeWorkspace){
    activeWorkspace=active;
    for(const node of next.querySelectorAll<HTMLDetailsElement>('details[data-active-branch="true"]')) {
      let ancestor:Element|null=node;
      while(ancestor && next.contains(ancestor)){
        if(ancestor instanceof HTMLDetailsElement) expanded.set(ancestor.id,true);
        ancestor=ancestor.parentElement;
      }
    }
  }
  for (const [id,open] of expanded) {const node=document.getElementById(id);if(node instanceof HTMLDetailsElement && next.contains(node) && node.open!==open) node.open=open;}
}).observe(document.body, {childList:true, subtree:true});
document.addEventListener('keydown', event => {
  if (event.key !== '/' || event.ctrlKey || event.metaKey || event.altKey) return;
  if ((event.target as Element)?.closest('input,textarea,select,code-input,[contenteditable]')) return;
  const search=document.querySelector<HTMLInputElement>('#workspace input[type="search"]');
  if (search) {event.preventDefault();search.focus();}
});
