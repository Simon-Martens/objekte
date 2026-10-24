import { execFileSync } from 'node:child_process';
import { mkdirSync, copyFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

const root = fileURLToPath(new URL('../', import.meta.url));
execFileSync('cargo', ['build', '--locked', '-p', 'structured-editor-search', '--target', 'wasm32-unknown-unknown', '--release'], { cwd: root, stdio: 'inherit' });
const { target_directory: target } = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1'], { cwd: root, encoding: 'utf8' }));
const destination = join(root, 'crates/server/assets/src/generated');
mkdirSync(destination, { recursive: true });
copyFileSync(join(target, 'wasm32-unknown-unknown/release/structured_editor_search.wasm'), join(destination, 'search.wasm'));
