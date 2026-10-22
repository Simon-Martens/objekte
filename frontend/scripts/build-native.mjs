import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../..');
const result = spawnSync('cargo', ['build', '-p', 'structured-editor-node', ...(process.argv.includes('--release') ? ['--release'] : [])], { cwd: root, stdio: 'inherit' });
if (result.status !== 0) process.exit(result.status ?? 1);
const profile = process.argv.includes('--release') ? 'release' : 'debug';
const filename = process.platform === 'win32' ? 'structured_editor_node.dll' : process.platform === 'darwin' ? 'libstructured_editor_node.dylib' : 'libstructured_editor_node.so';
const destination = resolve(import.meta.dirname, '../native/structured_editor_node.node');
mkdirSync(resolve(import.meta.dirname, '../native'), { recursive: true });
copyFileSync(resolve(root, 'target', profile, filename), destination);
console.log(`Native module: ${destination}`);
