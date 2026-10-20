import { bounded } from '../media/server.mjs';
import { failure } from './common.mjs';

export class Browsers {
  constructor({journal,url,token,control,fetcher=fetch}){Object.assign(this,{journal,url,token,control,fetcher});this.active=new Map();}
  async call(request,signal) {
    const response=await this.fetcher(`${this.url}/browser`,{method:'POST',headers:{authorization:`Bearer ${this.token}`,'content-type':'application/json'},body:JSON.stringify(request),signal,redirect:'error'});
    if(!response.ok)throw new Error('browser service failed');return JSON.parse(await bounded(response.body,2*1024*1024));
  }
  async invoke(operation,context,request,_admitted,signal) {
    if(operation!=='browse')return failure('unsupported browser operation');
    const id=context.parameters.authorization;
    const controller=new AbortController();this.active.set(request.call_id,{grant:id,controller});
    try {
      if(await this.journal.get(`revoked:${id}`))return failure('browser binding was revoked; no action dispatched');
      await this.journal.set(id,{id,grant:id,tenant:context.tenant,session:context.session,provider:'browser'});
      return await this.call(request,AbortSignal.any([signal,controller.signal]));
    } finally {this.active.delete(request.call_id);controller.abort();}
  }
  async maintain() {
    const pending=await this.control('pending');
    for(const grant of pending.grants)if(grant.provider==='browser') {
      await this.journal.set(`revoked:${grant.authorization}`,{id:`revoked:${grant.authorization}`,phase:'revoked'});
      for(const pending of this.active.values())if(pending.grant===grant.authorization)pending.controller.abort();
      if(await this.journal.get(grant.authorization)) {
        const result=await this.call({tenant_id:grant.tenant,session_id:grant.session,call_id:`cleanup:${grant.authorization}`,arguments:{action:'close'}},AbortSignal.timeout(30000));
        if(result.is_error)throw new Error('browser cleanup failed');await this.journal.delete(grant.authorization);
      }
      await this.control('cleaned',{authorization:grant.authorization,tenant:grant.tenant,session:grant.session});
    }
  }
}
