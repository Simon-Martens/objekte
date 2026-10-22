export type Issue = { path: string; message: string };
export class ApiFailure extends Error {
  constructor(message: string, public status: number, public issues: Issue[] = []) { super(message); }
}
let csrf = '';
export function setCsrf(value: string) { csrf = value; }
export async function api<T>(path: string, options: RequestInit = {}): Promise<T> {
  const headers = new Headers(options.headers);
  if (options.body && !(options.body instanceof FormData)) headers.set('Content-Type', 'application/json');
  if (options.method && !['GET', 'HEAD'].includes(options.method.toUpperCase()) && csrf) headers.set('X-CSRF-Token', csrf);
  const response = await fetch(`/api${path}`, { credentials: 'same-origin', ...options, headers });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new ApiFailure(data.error || `HTTP ${response.status}`, response.status, data.issues || []);
  return data as T;
}
export function jsonBody(value: unknown) { return JSON.stringify(value); }
export type User = { id: number; email: string; display_name: string; role: 'user' | 'editor' | 'administrator'; disabled: boolean };
export type Collection = { name: string; label: string; format: string; display: string; displayFields?: string[]; displayFallback: string[]; showInSidebar?: boolean; categories: { field: string; label: string; itemLabel?: string; display?: string; collection?: string; parentField?: string; descriptionField?: string; orderField?: string }[] };
export type CategoryNode = { value: string; label: string; count: number; description?: string | null; order?: number | null; children: CategoryNode[] };
export type RecordSummary = { id: string; label: string };
export type Reference = { collection: string; record_id: string; path: string };
export type Document = { id: string; value: any; revision: string };
export type MediaEntry = { name: string; path: string; kind: 'file' | 'directory'; size: number };
