export function unescapeInlineMarkdown(value: string) {
  return value.replace(/\\([\x21-\x2f\x3a-\x40\x5b-\x60\x7b-\x7e])/g, '$1');
}

export function escapeInlineMarkdown(value: string) {
  let result = '';
  for (let index = 0; index < value.length; index++) {
    const char = value[index];
    const prefix = value.slice(0, index);
    const listMarker = (char === '.' || char === ')') && /^\s*\d+$/.test(prefix) && /\s/.test(value[index + 1] ?? '');
    if ('\\`*_[]<>'.includes(char) || (prefix.trim() === '' && '#+-'.includes(char)) || listMarker) result += '\\';
    result += char;
  }
  return result;
}
