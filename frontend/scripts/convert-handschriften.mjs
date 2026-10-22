import { readProjectDefinition, writeProjectDefinition } from './project-config.mjs';
import { existsSync, mkdirSync, renameSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { convertHandschriften } from './handschriften.mjs';

const source = resolve(process.argv[2] ?? '../../lenz-archiv');
const project = resolve(process.argv[3] ?? '../examples/lenz-archiv');
const replace = process.argv.includes('--replace');
const schemaPath = join(project, '.editor/repository.json');
if (!existsSync(schemaPath)) throw new Error(`Not an editor project: ${project}`);

const converted = convertHandschriften(source);
const updates = [
  ['handschriften', converted.records, converted.schema],
  ['orte', converted.places, converted.placeSchema],
  ['sammlungen', converted.subcollections, converted.subcollectionSchema],
];
const schema = readProjectDefinition(project);
const variants = schema.anyOf ?? schema.oneOf;
if (!Array.isArray(variants)) throw new Error('Project schema needs a top-level anyOf or oneOf');
for (const [name, , branch] of updates) {
  const index = variants.findIndex(item => item['x-collection']?.name === name);
  if (index >= 0) variants[index] = branch;
  else variants.push(branch);
}
if (!replace && updates.some(([name]) => existsSync(join(project, 'data', name)))) {
  throw new Error('Target collections already exist; pass --replace to replace them');
}

const suffix = `${process.pid}-${Date.now()}`;
const stages = updates.map(([name, records]) => ({
  name, records, target: join(project, 'data', name),
  staging: join(project, 'data', `.${name}-${suffix}`),
  backup: join(project, 'data', `.${name}-backup-${suffix}`),
}));

try {
  for (const item of stages) {
    mkdirSync(item.staging, { recursive: true });
    for (const { id, value } of item.records) writeFileSync(join(item.staging, `${id}.json`), `${JSON.stringify(value, null, 2)}\n`);
    if (readdirSync(item.staging).length !== item.records.length) throw new Error(`Staging ${item.name} count mismatch`);
  }
  for (const item of stages) {
    if (existsSync(item.target)) renameSync(item.target, item.backup);
    renameSync(item.staging, item.target);
  }
  writeProjectDefinition(project, schema);
  for (const item of stages) rmSync(item.backup, { recursive: true, force: true });
  console.log(JSON.stringify({ ...converted.stats, places: converted.places.length, subcollections: converted.subcollections.length }, null, 2));
} catch (error) {
  for (const item of stages.reverse()) {
    if (existsSync(item.backup)) {
      rmSync(item.target, { recursive: true, force: true });
      renameSync(item.backup, item.target);
    } else if (existsSync(item.target) && !existsSync(item.staging)) {
      rmSync(item.target, { recursive: true, force: true });
    }
    rmSync(item.staging, { recursive: true, force: true });
  }
  throw error;
}
