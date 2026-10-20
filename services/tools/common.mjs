import { createHmac, timingSafeEqual } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';

export const result=value=>({content:[{type:'text',text:JSON.stringify(value)}],structured_content:value});
export const failure=(text,unknown=false)=>({content:[{type:'text',text}],is_error:true,unknown_outcome:unknown});
export function authenticate(header,key) {
  const token=header?.replace(/^Bearer /,''),parts=token?.split('.');
  if(parts?.length!==2 || token.length>16384)throw new Error('unauthorized');
  const expected=createHmac('sha256',key).update(parts[0]).digest(),actual=Buffer.from(parts[1],'base64url');
  if(actual.length!==expected.length || !timingSafeEqual(actual,expected))throw new Error('unauthorized');
  return JSON.parse(Buffer.from(parts[0],'base64url').toString('utf8'));
}
export function controller(origin,token,fetcher=fetch) {
  const url=new URL(origin);
  if(url.protocol!=='http:' || !['127.0.0.1','[::1]'].includes(url.hostname) || url.pathname!=='/' || url.username || url.password || url.search || url.hash || typeof token!=='string' || token.length<32)throw new Error('private tool authority required');
  return async(path,body)=>{
    const response=await fetcher(`${url.origin}/tools/${path}`,{method:body===undefined?'GET':'POST',headers:{authorization:`Bearer ${token}`,'content-type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)}),signal:AbortSignal.timeout(30000),redirect:'error'});
    if(!response.ok)throw new Error('tool authorization or reporting failed');return response.json();
  };
}
export class Journal {
  constructor(directory) {
    mkdirSync(directory,{recursive:true,mode:0o700});
    this.db=new DatabaseSync(join(directory,'tools.sqlite'));
    this.db.exec('PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS jobs (id TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE INDEX IF NOT EXISTS jobs_completion ON jobs (json_extract(value,\'$.verifier\')); CREATE INDEX IF NOT EXISTS jobs_grant ON jobs (json_extract(value,\'$.grant\'));');
  }
  get(id){const row=this.db.prepare('SELECT value FROM jobs WHERE id=?').get(id);return row?JSON.parse(row.value):undefined;}
  set(id,value){if(value.grant && this.get(`revoked:${value.grant}`))throw new Error('tool binding was revoked');this.db.prepare('INSERT INTO jobs(id,value) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET value=EXCLUDED.value').run(id,JSON.stringify(value));}
  values(){return this.db.prepare('SELECT value FROM jobs').all().map(row=>JSON.parse(row.value));}
  delete(id){this.db.prepare('DELETE FROM jobs WHERE id=?').run(id);}
  completion(verifier){const row=this.db.prepare("SELECT value FROM jobs WHERE json_extract(value,'$.verifier')=?").get(verifier);return row?JSON.parse(row.value):undefined;}
  deleteGrant(grant){this.db.prepare("DELETE FROM jobs WHERE json_extract(value,'$.grant')=?").run(grant);}
  close(){this.db.close();}
}
export class RemoteJournal {
  constructor(control){this.control=control;}
  async get(id){return await this.control('journal',{action:'get',id})??undefined;}
  set(id,value){return this.control('journal',{action:'set',id,value});}
  async completion(verifier){return await this.control('journal',{action:'completion',verifier})??undefined;}
  delete(id){return this.control('journal',{action:'delete',id});}
  deleteGrant(grant){return this.control('journal',{action:'delete_grant',grant});}
  close(){}
}
