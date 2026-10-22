import Database from 'better-sqlite3';
import argon2 from 'argon2';
import { createHash, randomBytes } from 'node:crypto';
import { mkdirSync, realpathSync } from 'node:fs';
import { resolve, sep } from 'node:path';
import { projectRoot } from './project';
import { normalizeEmail } from './email';

export class HttpError extends Error {
  constructor(public status: number, message: string, public issues: {path:string;message:string}[] = []) { super(message); }
}
export type User = { id: number; email: string; display_name: string; role: 'user'|'editor'|'administrator'; disabled: boolean };
export type Actor = { user: User; csrf: string };
const now = () => Math.floor(Date.now()/1000);
const secret = () => randomBytes(32).toString('hex');
const digest = (s:string) => createHash('sha256').update(s).digest('hex');
const roles = ['user','editor','administrator'];
function validRole(role:string) { if (!roles.includes(role)) throw new HttpError(400,'invalid role'); }
function email(input:string):string { try { return normalizeEmail(input); } catch { throw new HttpError(400,'enter a valid email address'); } }
function accountInsertError(error:unknown):never {
  if (error instanceof Database.SqliteError && error.code.startsWith('SQLITE_CONSTRAINT')) throw new HttpError(409,'email address already in use');
  throw error;
}
async function passwordHash(password:string) { if (password.length < 12) throw new HttpError(400,'password must contain at least 12 characters'); return argon2.hash(password,{type:argon2.argon2id}); }
const bool = (value:unknown) => Boolean(value);
let instance: Auth | undefined;
export function getAuth(): Auth { return instance ??= new Auth(); }

export class Auth {
  db: Database.Database;
  private attempts = new Map<string,{count:number;start:number}>();
  constructor() {
    const root = realpathSync(projectRoot());
    const directory = resolve(process.env.EDITOR_DATA_DIR ?? '../.data');
    mkdirSync(directory,{recursive:true});
    const state = realpathSync(directory);
    if (root === state || root.startsWith(state+sep) || state.startsWith(root+sep)) throw new Error('state and content directories must be separate');
    this.db = new Database(resolve(state,'editor.sqlite'));
    this.db.pragma('foreign_keys = ON'); this.db.pragma('journal_mode = WAL');
    const version = this.db.pragma('user_version',{simple:true}) as number;
    if (version !== 0 && version !== 2) throw new Error('unsupported development database; stop the server and run npm run dev:reset, then npm run admin:create');
    if (version === 0) this.db.exec(`BEGIN IMMEDIATE;
      CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL COLLATE NOCASE UNIQUE, display_name TEXT NOT NULL, password_hash TEXT NOT NULL, role TEXT NOT NULL CHECK(role IN ('user','editor','administrator')), disabled INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
      CREATE TABLE sessions (token_hash TEXT PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE, csrf TEXT NOT NULL, expires_at INTEGER NOT NULL);
      CREATE TABLE invitations (id INTEGER PRIMARY KEY, token_hash TEXT NOT NULL UNIQUE, role TEXT NOT NULL, email TEXT, expires_at INTEGER NOT NULL, redeemed INTEGER NOT NULL DEFAULT 0, revoked INTEGER NOT NULL DEFAULT 0);
      CREATE TABLE resets (token_hash TEXT PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE, expires_at INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0);
      CREATE INDEX sessions_user ON sessions(user_id); CREATE INDEX resets_user ON resets(user_id);
      PRAGMA user_version=2; COMMIT;`);
  }
  async bootstrap(address:string, displayName:string, password:string) {
    const normalized=email(address), hash=await passwordHash(password);
    if ((this.db.prepare("SELECT count(*) n FROM users WHERE role='administrator'").get() as any).n>0) throw new HttpError(400,'an administrator already exists');
    try { this.db.prepare("INSERT INTO users(email,display_name,password_hash,role,created_at) VALUES(?,?,?,'administrator',?)").run(normalized,displayName,hash,now()); }
    catch (error) { accountInsertError(error); }
  }
  async login(address:string,password:string):Promise<string> {
    const normalized=email(address);
    const attempt=this.attempts.get(normalized)??{count:0,start:now()};
    if (now()-attempt.start>300) { attempt.count=0; attempt.start=now(); }
    if (++attempt.count>10) throw new HttpError(429,'try again later');
    this.attempts.set(normalized,attempt);
    const row=this.db.prepare('SELECT id,password_hash,disabled FROM users WHERE email=?').get(normalized) as any;
    if (!row || bool(row.disabled) || !await argon2.verify(row.password_hash,password)) throw new HttpError(401,'authentication required');
    const token=secret(); this.db.prepare('INSERT INTO sessions(token_hash,user_id,csrf,expires_at) VALUES(?,?,?,?)').run(digest(token),row.id,secret(),now()+7*86400);
    this.attempts.delete(normalized); return token;
  }
  token(request:Request) { return request.headers.get('cookie')?.split(';').map(s=>s.trim()).find(s=>s.startsWith('editor_session='))?.slice('editor_session='.length); }
  actor(request:Request):Actor {
    const token=this.token(request); if (!token) throw new HttpError(401,'authentication required');
    const row=this.db.prepare('SELECT u.id,u.email,u.display_name,u.role,u.disabled,s.csrf FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=? AND s.expires_at>?').get(digest(token),now()) as any;
    if (!row || bool(row.disabled)) throw new HttpError(401,'authentication required');
    return {user:{id:row.id,email:row.email,display_name:row.display_name,role:row.role,disabled:bool(row.disabled)},csrf:row.csrf};
  }
  checked(request:Request,role:'user'|'editor'|'administrator',mutation=false):Actor {
    const actor=this.actor(request);
    if (roles.indexOf(actor.user.role)<roles.indexOf(role) || (mutation && request.headers.get('x-csrf-token')!==actor.csrf)) throw new HttpError(403,'permission denied or missing CSRF token');
    return actor;
  }
  cookie(token:string) { return `editor_session=${token}; Path=/api; HttpOnly; SameSite=Strict; Max-Age=604800${process.env.EDITOR_INSECURE_COOKIE==='1' || process.env.NODE_ENV!=='production' ? '' : '; Secure'}`; }
  clearCookie() { return `editor_session=; Path=/api; HttpOnly; SameSite=Strict; Max-Age=0${process.env.EDITOR_INSECURE_COOKIE==='1' || process.env.NODE_ENV!=='production' ? '' : '; Secure'}`; }
  logout(request:Request) { const token=this.token(request); if (token) this.db.prepare('DELETE FROM sessions WHERE token_hash=?').run(digest(token)); }
  users():User[] { return (this.db.prepare('SELECT id,email,display_name,role,disabled FROM users ORDER BY email').all() as any[]).map(r=>({...r,disabled:bool(r.disabled)})); }
  setUser(id:number,role?:string,disabled?:boolean) {
    if (role!==undefined) validRole(role);
    this.db.transaction(()=>{
      const row=this.db.prepare('SELECT role,disabled FROM users WHERE id=?').get(id) as any; if (!row) throw new HttpError(404,'not found');
      const newRole=role??row.role, newDisabled=disabled??bool(row.disabled);
      if (row.role==='administrator' && !bool(row.disabled) && (newRole!=='administrator'||newDisabled) && (this.db.prepare("SELECT count(*) n FROM users WHERE role='administrator' AND disabled=0").get() as any).n<=1) throw new HttpError(400,'cannot remove the final active administrator');
      this.db.prepare('UPDATE users SET role=?,disabled=? WHERE id=?').run(newRole,newDisabled?1:0,id);
      if (newDisabled) this.revokeSessions(id);
    })();
  }
  revokeSessions(id:number) { this.db.prepare('DELETE FROM sessions WHERE user_id=?').run(id); }
  invite(role:string,address:string|undefined,ttl:number):string {
    validRole(role); const normalized=address ? email(address) : null;
    const token=secret(); this.db.prepare('INSERT INTO invitations(token_hash,role,email,expires_at) VALUES(?,?,?,?)').run(digest(token),role,normalized,now()+Math.min(Math.max(ttl,60),30*86400)); return token;
  }
  invitations() { return (this.db.prepare('SELECT id,role,email,expires_at,redeemed,revoked FROM invitations ORDER BY id DESC').all() as any[]).map(r=>({...r,redeemed:bool(r.redeemed),revoked:bool(r.revoked)})); }
  revokeInvite(id:number) { this.db.prepare('UPDATE invitations SET revoked=1 WHERE id=?').run(id); }
  async redeem(token:string,address:string,displayName:string,password:string) {
    const normalized=email(address), hash=await passwordHash(password);
    try { this.db.transaction(()=>{
      const row=this.db.prepare('SELECT id,role,email FROM invitations WHERE token_hash=? AND expires_at>? AND redeemed=0 AND revoked=0').get(digest(token),now()) as any;
      if (!row) throw new HttpError(400,'invitation invalid or expired');
      if (row.email && row.email!==normalized) throw new HttpError(400,'email does not match invitation');
      this.db.prepare('INSERT INTO users(email,display_name,password_hash,role,created_at) VALUES(?,?,?,?,?)').run(normalized,displayName,hash,row.role,now());
      this.db.prepare('UPDATE invitations SET redeemed=1 WHERE id=?').run(row.id);
    })(); } catch (error) { accountInsertError(error); }
  }
  createReset(id:number):string { const token=secret(); const result=this.db.prepare('INSERT INTO resets(token_hash,user_id,expires_at) SELECT ?,id,? FROM users WHERE id=?').run(digest(token),now()+3600,id); if (!result.changes) throw new HttpError(404,'not found'); return token; }
  async redeemReset(token:string,password:string) {
    const hash=await passwordHash(password);
    this.db.transaction(()=>{
      const row=this.db.prepare('SELECT user_id FROM resets WHERE token_hash=? AND used=0 AND expires_at>?').get(digest(token),now()) as any;
      if (!row) throw new HttpError(400,'reset link invalid or expired');
      this.db.prepare('UPDATE users SET password_hash=? WHERE id=?').run(hash,row.user_id);
      this.revokeSessions(row.user_id); this.db.prepare('UPDATE resets SET used=1 WHERE user_id=?').run(row.user_id);
    })();
  }
  async changePassword(id:number,oldPassword:string,newPassword:string) {
    const hash=await passwordHash(newPassword), row=this.db.prepare('SELECT password_hash FROM users WHERE id=?').get(id) as any;
    if (!row || !await argon2.verify(row.password_hash,oldPassword)) throw new HttpError(401,'authentication required');
    this.db.transaction(()=>{this.db.prepare('UPDATE users SET password_hash=? WHERE id=?').run(hash,id);this.revokeSessions(id);})();
  }
}
