import { useEffect, useRef, useState } from 'react';
import Editor, { loader, type OnMount } from '@monaco-editor/react';
import * as monaco from 'monaco-editor/editor/editor.api.js';
import 'monaco-editor/languages/definitions/markdown/register.js';
import 'monaco-editor/languages/definitions/html/register.js';
import '@fontsource/jetbrains-mono/400.css';
import { RiLinkM as LinkIcon } from '@remixicon/react';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import MarkdownLinkPicker from '@/components/MarkdownLinkPicker';
import EditorWorker from 'monaco-editor/editor/editor.worker.js?worker';
import { escapeInlineMarkdown, unescapeInlineMarkdown } from '@/lib/inline-markdown';
import { linkAtSelection, type MarkdownLink } from '@/lib/markdown-links';

self.MonacoEnvironment = { getWorker: () => new EditorWorker() };
loader.config({ monaco });
monaco.editor.defineTheme('editor-code', {
  base: 'vs', inherit: true, rules: [],
  colors: {
    'editor.background': '#f8faf8',
    'editorGutter.background': '#eef2ee',
    'editorLineNumber.foreground': '#839187',
    'editorLineNumber.activeForeground': '#215b48',
    'editor.lineHighlightBackground': '#edf4ee',
  },
});

export default function CodeEditor({ id, label, value, onChange, language = 'markdown', singleLine = false, readOnly = false }: {
  id: string; label: string; value: string; onChange: (value: string) => void; language?: 'markdown' | 'html'; singleLine?: boolean; readOnly?: boolean;
}) {
  const [draft, setDraft] = useState(() => singleLine ? unescapeInlineMarkdown(value) : value);
  const [height, setHeight] = useState(singleLine ? 38 : 132);
  const [linkPickerOpen, setLinkPickerOpen] = useState(false);
  const [selectedText, setSelectedText] = useState('');
  const [existingLink, setExistingLink] = useState<MarkdownLink | null>(null);
  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const selectionRef = useRef<monaco.Range | null>(null);

  useEffect(() => {
    if (singleLine) setDraft(unescapeInlineMarkdown(value));
    else setDraft(value);
  }, [value, singleLine]);

  const onMount: OnMount = editor => {
    editorRef.current = editor;
    if (language === 'markdown' && !singleLine && !readOnly) editor.addAction({
      id: 'insert-markdown-link', label: 'Insert or edit link',
      keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyK],
      contextMenuGroupId: 'navigation',
      run: () => openLinkPicker(editor),
    });
    if (singleLine) editor.addCommand(monaco.KeyCode.Enter, () => {});
    else {
      const resize = () => setHeight(Math.min(360, Math.max(132, editor.getContentHeight() + 4)));
      editor.onDidContentSizeChange(resize);
      resize();
    }
  };

  const openLinkPicker = (editor = editorRef.current) => {
    if (!editor) return;
    const model = editor.getModel();
    const selection = editor.getSelection();
    if (!model || !selection) return;
    const existing = linkAtSelection(model.getValue(), model.getOffsetAt(selection.getStartPosition()), model.getOffsetAt(selection.getEndPosition()));
    selectionRef.current = existing
      ? monaco.Range.fromPositions(model.getPositionAt(existing.start), model.getPositionAt(existing.end))
      : selection;
    setExistingLink(existing);
    setSelectedText(existing?.label ?? model.getValueInRange(selection));
    setLinkPickerOpen(true);
  };

  const insertLink = (destination: string, label: string, image: boolean) => {
    const editor = editorRef.current;
    const model = editor?.getModel();
    if (!editor || !model) return;
    const range = selectionRef.current || editor.getSelection();
    if (!range) return;
    const text = label.replace(/\s+/g, ' ').replace(/[\\\[\]]/g, '\\$&');
    const target = destination.trim().replace(/[\s()<>\[\]]/g, character => encodeURIComponent(character));
    const insertion = `${image ? '!' : ''}[${text}](${target})`;
    const offset = model.getOffsetAt(range.getStartPosition());
    editor.pushUndoStop();
    editor.executeEdits('markdown-link-picker', [{ range, text: insertion, forceMoveMarkers: true }]);
    editor.pushUndoStop();
    editor.setPosition(model.getPositionAt(offset + insertion.length));
    requestAnimationFrame(() => editor.focus());
  };

  return <><div id={id} className="code-editor flex" onClick={() => editorRef.current?.focus()}><div className="min-w-0 flex-1">
    <Editor
      height={height}
      language={language}
      theme="editor-code"
      value={draft}
      loading={<div className="px-3 py-2 text-sm text-muted-foreground">Loading editor…</div>}
      onMount={onMount}
      onChange={next => {
        const text = singleLine ? (next ?? '').replace(/\r?\n/g, ' ') : next ?? '';
        setDraft(text);
        onChange(singleLine ? escapeInlineMarkdown(text) : text);
      }}
      options={{
        ariaLabel: label,
        automaticLayout: true,
        folding: false,
        fontFamily: '"JetBrains Mono", ui-monospace, monospace',
        fontSize: 13,
        glyphMargin: false,
        lineDecorationsWidth: 22, // Monaco's default 10px after the number, plus 12px before the text.
        lineNumbers: 'on',
        lineNumbersMinChars: 2,
        minimap: { enabled: false },
        overviewRulerLanes: 0,
        padding: { top: singleLine ? 8 : 4, bottom: singleLine ? 0 : 6 },
        readOnly,
        renderLineHighlight: 'line',
        renderLineHighlightOnlyWhenFocus: true,
        scrollBeyondLastLine: false,
        wordWrap: 'on',
      }}
    />
  </div>{language === 'markdown' && !singleLine && !readOnly && <Tooltip><TooltipTrigger asChild><button type="button" className="flex h-8 w-8 shrink-0 items-center justify-center text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" aria-label="Insert or edit link" onMouseDown={event => event.preventDefault()} onClick={event => { event.stopPropagation(); openLinkPicker(); }}><LinkIcon size={17} /></button></TooltipTrigger><TooltipContent side="top">Insert or edit link · Ctrl/Cmd+K</TooltipContent></Tooltip>}</div>
  {language === 'markdown' && !singleLine && !readOnly && <MarkdownLinkPicker open={linkPickerOpen} onOpenChange={setLinkPickerOpen} selectedText={selectedText} existingLink={existingLink} onInsert={insertLink} />}
  </>;
}
