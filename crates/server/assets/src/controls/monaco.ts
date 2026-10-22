// Loaded only when a source field is mounted. Vite serves every worker locally.
import * as monaco from 'monaco-editor/editor/editor.api.js';
// 0.57's HTML worker bridge imports these contributions lazily. Register their
// services before the first editor initializes Monaco's service collection.
import 'monaco-editor/internal/common/workers.js';
import 'monaco-editor/languages/definitions/markdown/register.js';
import 'monaco-editor/languages/definitions/html/register.js';
import 'monaco-editor/languages/features/html/register.js';
import 'monaco-editor/editor/contrib/tokenization/browser/tokenization.js';
import 'monaco-editor/editor/contrib/find/browser/findController.js';
import 'monaco-editor/editor/contrib/clipboard/browser/clipboard.js';
import 'monaco-editor/editor/contrib/contextmenu/browser/contextmenu.js';
import 'monaco-editor/editor/contrib/wordOperations/browser/wordOperations.js';
import 'monaco-editor/editor/contrib/linesOperations/browser/linesOperations.js';
import 'monaco-editor/editor/contrib/bracketMatching/browser/bracketMatching.js';
import 'monaco-editor/editor/contrib/hover/browser/hoverContribution.js';
import 'monaco-editor/editor/contrib/suggest/browser/suggestController.js';
import 'monaco-editor/editor/contrib/snippet/browser/snippetController2.js';
import 'monaco-editor/editor/contrib/format/browser/formatActions.js';
import 'monaco-editor/editor/contrib/toggleTabFocusMode/browser/toggleTabFocusMode.js';
import EditorWorker from 'monaco-editor/editor/editor.worker.js?worker';
import HtmlWorker from 'monaco-editor/languages/features/html/html.worker.js?worker';

self.MonacoEnvironment = {
  getWorker: (_id, label) => label === 'html' ? new HtmlWorker() : new EditorWorker(),
};
monaco.editor.defineTheme('objekte-light', {
  base: 'vs', inherit: true, rules: [],
  colors: {
    'editor.background': '#ffffff',
    'editor.foreground': '#292e35',
    'editorLineNumber.foreground': '#a0a7af',
    'editorLineNumber.activeForeground': '#58636f',
    'editor.lineHighlightBackground': '#f6f8fa',
    'editor.selectionBackground': '#d9e9e5',
    'editor.inactiveSelectionBackground': '#eaf0ee',
    'editorCursor.foreground': '#236957',
    'editorWidget.border': '#dce1e5',
    'editorWidget.background': '#ffffff',
    'focusBorder': '#538d7e',
  },
});
export { monaco };

monaco.editor.defineTheme('objekte-dark', {
  base: 'vs-dark', inherit: true, rules: [],
  colors: {
    'editor.background': '#191d24', 'editor.foreground': '#d6dbe3',
    'editorLineNumber.foreground': '#626c7b', 'editorLineNumber.activeForeground': '#aeb9c9',
    'editor.lineHighlightBackground': '#202630', 'editor.selectionBackground': '#294d48',
    'editor.inactiveSelectionBackground': '#27343b', 'editorCursor.foreground': '#81c4b4',
    'editorWidget.border': '#343c49', 'editorWidget.background': '#202630',
    'focusBorder': '#69b5a0',
  },
});
function syncTheme() { monaco.editor.setTheme(`objekte-${document.documentElement.dataset.theme || 'light'}`); }
window.addEventListener('theme-change', syncTheme);
syncTheme();
