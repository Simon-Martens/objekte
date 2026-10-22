import type { APIRoute } from 'astro';
import { getAuth, HttpError } from '../../server/auth';
import { getProject } from '../../server/project';

export const prerender = false;
const ok = {ok:true};
const json = (value:unknown,status=200,headers:HeadersInit={}) => new Response(JSON.stringify(value),{status,headers:{'Content-Type':'application/json; charset=utf-8','Cache-Control':'no-store',...headers}});
const number = (text:string|null,defaultValue:number) => text === null ? defaultValue : Math.max(0,Math.floor(Number(text)||0));
const parse = (raw:string) => JSON.parse(raw);
const now = () => Math.floor(Date.now()/1000);
async function body(request:Request):Promise<any> {
  if (Number(request.headers.get('content-length')??0)>24*1024*1024) throw new HttpError(413,'request body too large');
  const text=await request.text();
  if (text.length>24*1024*1024) throw new HttpError(413,'request body too large');
  try { return JSON.parse(text); } catch { throw new HttpError(400,'invalid JSON body'); }
}
function required(input:any,...fields:string[]) {
  if (!input || typeof input!=='object' || fields.some(field => typeof input[field]!=='string')) throw new HttpError(400,`required fields: ${fields.join(', ')}`);
}
function nativeError(error:unknown):HttpError {
  if (error instanceof HttpError) return error;
  try {
    const data=JSON.parse((error as Error).message);
    return new HttpError(({validation:422,conflict:409,not_found:404,bad_request:400,internal:500} as Record<string,number>)[data.code]??500,data.error??'error',data.issues??[]);
  } catch { return new HttpError(500,(error as Error)?.message??'internal error'); }
}
const mime = (path:string) => ({png:'image/png',jpg:'image/jpeg',jpeg:'image/jpeg',gif:'image/gif',webp:'image/webp',svg:'image/svg+xml',pdf:'application/pdf',mp3:'audio/mpeg',mp4:'video/mp4',txt:'text/plain'} as Record<string,string>)[path.split('.').pop()?.toLowerCase()??'']??'application/octet-stream';
async function handle(request:Request,path:string):Promise<Response> {
  const method=request.method;
  const parts=path.split('/').filter(Boolean).map(decodeURIComponent);
  const query=new URL(request.url).searchParams;
  const auth=getAuth();
  if (method==='GET' && path==='health') return json(ok);
  if (parts[0]==='auth') {
    if (method==='POST' && parts[1]==='login') { const input=await body(request); required(input,'email','password'); const token=await auth.login(input.email,input.password); return json(ok,200,{'Set-Cookie':auth.cookie(token)}); }
    if (method==='POST' && parts[1]==='redeem-invitation') { const input=await body(request); required(input,'token','email','display_name','password'); await auth.redeem(input.token,input.email,input.display_name,input.password); return json(ok); }
    if (method==='POST' && parts[1]==='reset-password') { const input=await body(request); required(input,'token','password'); await auth.redeemReset(input.token,input.password); return json(ok); }
    if (method==='GET' && parts[1]==='me') { const actor=auth.checked(request,'user'); return json({user:actor.user,csrf:actor.csrf}); }
    if (method==='POST' && parts[1]==='logout') { auth.checked(request,'user',true); auth.logout(request); return json(ok,200,{'Set-Cookie':auth.clearCookie()}); }
    if (method==='POST' && parts[1]==='password') { const actor=auth.checked(request,'user',true),input=await body(request); required(input,'old_password','new_password'); await auth.changePassword(actor.user.id,input.old_password,input.new_password); return json(ok,200,{'Set-Cookie':auth.clearCookie()}); }
  }
  if (parts[0]==='collections') {
    if (method==='GET' && parts.length===1) { auth.checked(request,'user'); return json(parse(getProject().collections())); }
    if (method==='GET' && parts.length===3 && parts[2]==='schema') { auth.checked(request,'user'); return json(parse(getProject().schema(parts[1]))); }
    if (method==='GET' && parts.length===3 && parts[2]==='categories') { auth.checked(request,'user'); return json(parse(getProject().categoryTree(parts[1]))); }
    if (method==='GET' && parts.length===3 && parts[2]==='field-values') { auth.checked(request,'user'); const field=query.get('field'); if (!field) throw new HttpError(400,'field required'); return json(parse(getProject().fieldValues(parts[1],field,query.get('q')??'',query.get('scope')??'{}',number(query.get('limit'),50)))); }
    if (parts.length===4 && parts[2]==='ids' && parts[3]==='check' && method==='GET') { auth.checked(request,'editor'); return json(parse(getProject().validateNewId(parts[1],query.get('id')??''))); }
    if (parts.length===4 && parts[2]==='ids' && parts[3]==='generate' && method==='POST') { auth.checked(request,'editor',true); const input=await body(request); if (input?.value===undefined) throw new HttpError(400,'value required'); return json({id:parse(getProject().generateId(parts[1],JSON.stringify(input.value)))}); }
    if (parts.length===3 && parts[2]==='records') {
      if (method==='GET') { auth.checked(request,'user'); const category=query.getAll('category'); return json(parse(category.length
        ? getProject().recordsInCategory(parts[1],query.get('q')??'',category,number(query.get('offset'),0),number(query.get('limit'),50))
        : getProject().records(parts[1],query.get('q')??'',number(query.get('offset'),0),number(query.get('limit'),50)))); }
      if (method==='POST') { auth.checked(request,'editor',true); const input=await body(request); required(input,'id'); if (input.value===undefined) throw new HttpError(400,'value required'); return json(parse(getProject().save(parts[1],input.id,JSON.stringify(input.value))),201); }
    }
    if (parts.length===4 && parts[2]==='records') {
      if (method==='GET') { auth.checked(request,'user'); return json(parse(getProject().record(parts[1],parts[3]))); }
      if (method==='PUT') { auth.checked(request,'editor',true); const input=await body(request); required(input,'revision'); if (input.value===undefined) throw new HttpError(400,'value required'); if (input.id!==undefined && typeof input.id!=='string') throw new HttpError(400,'id must be a string'); return json(parse(input.id && input.id!==parts[3]
        ? getProject().rename(parts[1],parts[3],input.id,JSON.stringify(input.value),input.revision)
        : getProject().save(parts[1],parts[3],JSON.stringify(input.value),input.revision))); }
      if (method==='DELETE') { auth.checked(request,'editor',true); const input=await body(request); required(input,'revision'); return json(parse(getProject().delete(parts[1],parts[3],input.revision))); }
    }
  }
  if (parts[0]==='relations' && parts.length===3 && parts[2]==='search' && method==='GET') {
    auth.checked(request,'user'); const search=query.get('search')?.split(',').filter(Boolean);
    return json(parse(getProject().relationSearch(parts[1],query.get('q')??'',number(query.get('offset'),0),number(query.get('limit'),20),query.get('display')??undefined,search)));
  }
  if (parts[0]==='relations' && parts.length===3 && parts[2]!=='search' && method==='GET') {
    auth.checked(request,'user');
    return json(parse(getProject().relationResolve(parts[1],parts[2])));
  }
  if (parts[0]==='relations' && parts.length===4 && parts[3]==='references' && method==='GET') {
    auth.checked(request,'user');
    return json(parse(getProject().referencesTo(parts[1],parts[2])));
  }
  if (parts[0]==='media') {
    if (method==='GET' && parts.length===1) { auth.checked(request,'user'); return json(parse(getProject().mediaList(query.get('path')??'',number(query.get('offset'),0),number(query.get('limit'),50)))); }
    if (method==='GET' && parts[1]==='search') { auth.checked(request,'user'); return json(parse(getProject().mediaSearch(query.get('q')??'',number(query.get('offset'),0),number(query.get('limit'),50)))); }
    if (method==='GET' && parts[1]==='references') { auth.checked(request,'user'); const file=query.get('path'); if (!file) throw new HttpError(400,'path required'); return json(parse(getProject().mediaReferences(file))); }
    if (method==='GET' && parts[1]==='file') {
      auth.checked(request,'user'); const file=query.get('path'); if (!file) throw new HttpError(400,'path required');
      const bytes=getProject().mediaRead(file); const type=mime(file); const inline=['image/png','image/jpeg','image/gif','image/webp'].includes(type);
      return new Response(new Uint8Array(bytes),{headers:{'Content-Type':type,'X-Content-Type-Options':'nosniff','Content-Disposition':inline?'inline':'attachment','Cache-Control':'private, no-store'}});
    }
    if (method==='POST' && parts[1]==='upload') {
      auth.checked(request,'editor',true); if (Number(request.headers.get('content-length')??0)>24*1024*1024) throw new HttpError(413,'request body too large');
      const form=await request.formData(); const file=form.get('file'), directory=form.get('directory');
      if (!(file instanceof File)) throw new HttpError(400,'file required');
      const path=[typeof directory==='string'?directory:'',file.name].filter(Boolean).join('/');
      getProject().mediaUpload(path,Buffer.from(await file.arrayBuffer()),20*1024*1024); return json({path});
    }
    if (method==='POST' && ['mkdir','move','delete'].includes(parts[1])) {
      auth.checked(request,'editor',true); const input=await body(request);
      if (parts[1]==='mkdir') { required(input,'path'); getProject().mediaMkdir(input.path); }
      if (parts[1]==='move') { required(input,'source','target'); getProject().mediaMove(input.source,input.target); }
      if (parts[1]==='delete') { required(input,'path'); getProject().mediaDelete(input.path); }
      return json(ok);
    }
  }
  if (parts[0]==='admin') {
    if (parts[1]==='users') {
      if (method==='GET' && parts.length===2) { auth.checked(request,'administrator'); return json(auth.users()); }
      if (parts.length>=3) {
        const id=Number(parts[2]); if (!Number.isSafeInteger(id)||id<1) throw new HttpError(400,'invalid user ID');
        if (method==='PUT' && parts.length===3) { auth.checked(request,'administrator',true); const input=await body(request); auth.setUser(id,input.role,input.disabled); return json(ok); }
        if (method==='POST' && parts[3]==='reset') { auth.checked(request,'administrator',true); return json({token:auth.createReset(id),expires_at:now()+3600}); }
        if (method==='POST' && parts[3]==='revoke-sessions') { auth.checked(request,'administrator',true); auth.revokeSessions(id); return json(ok); }
      }
    }
    if (parts[1]==='invitations') {
      if (method==='GET' && parts.length===2) { auth.checked(request,'administrator'); return json(auth.invitations()); }
      if (method==='POST' && parts.length===2) { auth.checked(request,'administrator',true); const input=await body(request); required(input,'role'); const hours=Math.min(Math.max(Number(input.expires_hours)||24,1),720); return json({token:auth.invite(input.role,input.email,hours*3600),expires_at:now()+hours*3600}); }
      if (method==='POST' && parts[3]==='revoke') { auth.checked(request,'administrator',true); auth.revokeInvite(Number(parts[2])); return json(ok); }
    }
  }
  throw new HttpError(404,'not found');
}
const route:APIRoute=async ({request,params})=>{
  try { return await handle(request,params.path??''); }
  catch(error) { const e=nativeError(error); if (e.status===500) console.error(error); return json({error:e.message,issues:e.issues},e.status); }
};
export const GET=route, POST=route, PUT=route, DELETE=route;
