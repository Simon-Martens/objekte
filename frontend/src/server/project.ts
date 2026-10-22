import { createRequire } from 'node:module';
import { resolve } from 'node:path';

export interface NativeProject {
  documentMappings(): string;
  documentSchema(name: string): string;
  documentFiles(name: string): string;
  jsonDocument(name: string, path: string): string;
  saveJsonDocument(name: string, path: string, value: string, revision: string): string;
  uiConfig(): string;
  settingsSchema(): string;
  settings(): string;
  saveSettings(value: string, revision: string): string;
  collections(): string;
  projectSchema(): string;
  schema(collection: string): string;
  records(collection: string, query: string, offset: number, limit: number): string;
  categoryTree(collection: string): string;
  recordsInCategory(collection: string, query: string, category: string[], offset: number, limit: number): string;
  record(collection: string, id: string): string;
  validateNewId(collection: string, id: string): string;
  generateId(collection: string, value: string): string;
  fieldValues(collection: string, field: string, query: string, scope: string, limit: number): string;
  validate(collection: string, value: string): string;
  save(collection: string, id: string, value: string, revision?: string): string;
  rename(collection: string, oldId: string, newId: string, value: string, revision: string): string;
  delete(collection: string, id: string, revision: string): string;
  relationSearch(collection: string, query: string, offset: number, limit: number, display?: string, search?: string[]): string;
  relationResolve(collection: string, id: string, display?: string): string;
  referencesTo(collection: string, id: string): string;
  mediaList(path: string, offset: number, limit: number): string;
  mediaSearch(query: string, offset: number, limit: number): string;
  mediaReferences(path: string): string;
  mediaRead(path: string): Buffer;
  mediaUpload(path: string, bytes: Buffer, maxBytes: number): void;
  mediaMkdir(path: string): void;
  mediaMove(source: string, target: string): void;
  mediaDelete(path: string): void;
}

let project: NativeProject | undefined;
export function projectRoot(): string { return resolve(process.env.EDITOR_ROOT ?? '../examples/lenz-archiv'); }
export function getProject(): NativeProject {
  if (!project) {
    const require = createRequire(import.meta.url);
    const modulePath = resolve(process.env.EDITOR_NATIVE_MODULE ?? './native/structured_editor_node.node');
    const native = require(modulePath);
    project = new native.Project(projectRoot()) as NativeProject;
  }
  return project;
}
