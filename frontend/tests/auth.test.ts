import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import Database from 'better-sqlite3';
import { Auth, HttpError } from '../src/server/auth';

const password = 'a long test password';

test('email addresses identify accounts from bootstrap through invitation and login', async () => {
  const state = mkdtempSync(join(tmpdir(), 'editor-email-test-'));
  process.env.EDITOR_DATA_DIR = state;
  process.env.EDITOR_ROOT = resolve('../examples/project');
  const auth = new Auth();
  try {
    assert.equal(auth.db.pragma('user_version', {simple:true}), 2);
    await assert.rejects(auth.bootstrap('admin', 'Admin', password), (error:unknown) => error instanceof HttpError && error.status === 400);
    await auth.bootstrap('Admin@Example.COM', 'Admin', password);
    assert.equal(auth.users()[0].email, 'admin@example.com');
    const token = await auth.login('ADMIN@example.com', password);
    const actor = auth.actor(new Request('http://localhost/api/auth/me', {headers:{cookie:`editor_session=${token}`}}));
    assert.equal(actor.user.email, 'admin@example.com');
    assert.throws(() => auth.invite('editor', 'not-an-email', 3600), (error:unknown) => error instanceof HttpError && error.status === 400);
    const invite = auth.invite('editor', 'Editor@Example.COM', 3600);
    await assert.rejects(auth.redeem(invite, 'other@example.com', 'Editor', password), (error:unknown) => error instanceof HttpError && error.status === 400);
    await auth.redeem(invite, 'EDITOR@example.com', 'Editor', password);
    assert.equal(auth.users().find(user => user.role === 'editor')?.email, 'editor@example.com');
    const duplicate = auth.invite('editor', undefined, 3600);
    await assert.rejects(auth.redeem(duplicate, 'EDITOR@example.com', 'Duplicate', password), (error:unknown) => error instanceof HttpError && error.status === 409);
  } finally {
    auth.db.close();
    rmSync(state, {recursive:true, force:true});
  }
});

test('older development databases are not migrated', () => {
  const state = mkdtempSync(join(tmpdir(), 'editor-old-state-test-'));
  process.env.EDITOR_DATA_DIR = state;
  process.env.EDITOR_ROOT = resolve('../examples/project');
  const db = new Database(join(state, 'editor.sqlite'));
  db.pragma('user_version = 1');
  db.close();
  try { assert.throws(() => new Auth(), /npm run dev:reset/); }
  finally { rmSync(state, {recursive:true, force:true}); }
});
