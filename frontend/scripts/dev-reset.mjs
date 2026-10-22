import { existsSync, lstatSync, realpathSync, rmSync, unlinkSync } from 'node:fs';
import { dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

if (process.env.NODE_ENV === 'production') {
  throw new Error('dev:reset cannot run with NODE_ENV=production');
}

const frontend = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const project = realpathSync(resolve(process.env.EDITOR_ROOT ?? join(frontend, '../examples/lenz-archiv')));
const dataDirectory = resolve(process.env.EDITOR_DATA_DIR ?? join(frontend, '../.data'));

if (existsSync(dataDirectory)) {
  const state = realpathSync(dataDirectory);
  if (project === state || project.startsWith(state + sep) || state.startsWith(project + sep)) {
    throw new Error('state and content directories must be separate');
  }
}

const databaseFiles = ['editor.sqlite-wal', 'editor.sqlite-shm', 'editor.sqlite-journal', 'editor.sqlite'];
for (const name of databaseFiles) {
  const path = join(dataDirectory, name);
  if (!existsSync(path)) continue;
  if (!lstatSync(path).isFile()) throw new Error(`refusing to remove non-file: ${path}`);
  unlinkSync(path);
  console.log(`Removed ${path}`);
}

for (const name of ['.astro', 'dist']) {
  const path = join(frontend, name);
  if (!existsSync(path)) continue;
  if (!lstatSync(path).isDirectory()) throw new Error(`refusing to remove non-directory: ${path}`);
  rmSync(path, { recursive: true });
  console.log(`Removed ${path}`);
}

console.log('Development state reset. Run npm run admin:create, then npm run dev.');
