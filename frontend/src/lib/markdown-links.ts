export type MarkdownLink = {
  start: number;
  end: number;
  label: string;
  destination: string;
  image: boolean;
};

function escaped(text: string, index: number) {
  let slashes = 0;
  for (let position = index - 1; text[position] === '\\'; position--) slashes++;
  return slashes % 2 === 1;
}

export function linkAtSelection(text: string, start: number, end: number): MarkdownLink | null {
  for (let index = 0; index < text.length; index++) {
    if (text[index] !== '[' || escaped(text, index)) continue;
    const image = index > 0 && text[index - 1] === '!' && !escaped(text, index - 1);
    const linkStart = image ? index - 1 : index;
    let closeLabel = index + 1;
    let depth = 1;
    for (; closeLabel < text.length; closeLabel++) {
      if (escaped(text, closeLabel)) continue;
      if (text[closeLabel] === '[') depth++;
      if (text[closeLabel] === ']' && --depth === 0) break;
    }
    if (depth !== 0 || text[closeLabel + 1] !== '(') continue;
    let closeTarget = closeLabel + 2;
    depth = 1;
    for (; closeTarget < text.length; closeTarget++) {
      if (escaped(text, closeTarget)) continue;
      if (text[closeTarget] === '(') depth++;
      if (text[closeTarget] === ')' && --depth === 0) break;
    }
    if (depth !== 0) continue;
    const linkEnd = closeTarget + 1;
    const containsCursor = start === end && start >= linkStart && start <= linkEnd;
    const overlapsSelection = start !== end && start < linkEnd && end > linkStart;
    if (containsCursor || overlapsSelection) return {
      start: linkStart,
      end: linkEnd,
      label: text.slice(index + 1, closeLabel).replace(/\\([\\\[\]])/g, '$1'),
      destination: text.slice(closeLabel + 2, closeTarget),
      image,
    };
    index = closeTarget;
  }
  return null;
}
