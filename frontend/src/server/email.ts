import { domainToASCII } from 'node:url';

/** The canonical account identifier. Product accounts use unquoted ASCII local parts. */
export function normalizeEmail(input: string): string {
  const value = input.trim();
  const at = value.indexOf('@');
  if (at < 1 || at !== value.lastIndexOf('@')) throw new Error('enter a valid email address');
  const local = value.slice(0, at).toLowerCase();
  const domain = domainToASCII(value.slice(at + 1)).toLowerCase();
  if (local.length > 64 || !/^[a-z0-9!#$%&'*+/=?^_`{|}~.-]+$/.test(local)
    || local.startsWith('.') || local.endsWith('.') || local.includes('..')) {
    throw new Error('enter a valid email address');
  }
  const labels = domain.split('.');
  if (labels.length < 2 || labels.some(label => label.length < 1 || label.length > 63
    || !/^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label))
    || `${local}@${domain}`.length > 254) {
    throw new Error('enter a valid email address');
  }
  return `${local}@${domain}`;
}
