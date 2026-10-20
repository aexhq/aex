import { createHash, randomBytes } from 'node:crypto';
import { publicJSON } from './outbound.mjs';
import { failure } from './common.mjs';
import { isDeepStrictEqual } from 'node:util';

export class Applications {
  constructor({journal,callbackOrigin,outbound=publicJSON,now=Date.now}){
    const origin=new URL(callbackOrigin);if(origin.origin!==callbackOrigin || origin.protocol!=='https:')throw new Error('application completion requires an HTTPS public origin');
    Object.assign(this,{journal,callbackOrigin,outbound,now});this.waiters=new Map();this.active=new Map();
  }
  async invoke(operation,context,request,admitted,signal,progress=()=>{}) {
    const key=`application:${context.session}:${request.call_id}`,grant=context.parameters.authorization,controller=new AbortController();
    const abort=()=>controller.abort(signal.reason);signal.addEventListener('abort',abort,{once:true});if(signal.aborted)abort();
    const deadline=Math.min(admitted.expires_ms,this.now()+admitted.parameters.timeout_ms),timer=setTimeout(()=>controller.abort(),Math.max(0,deadline-this.now()));
    this.active.set(key,{grant,controller});
    try {return await this.dispatch(operation,context,request,admitted,controller.signal,progress,deadline);}
    finally {clearTimeout(timer);signal.removeEventListener('abort',abort);controller.abort();this.active.delete(key);}
  }
  async dispatch(operation,context,request,admitted,signal,progress,deadline) {
    const key=`application:${context.session}:${request.call_id}`;
    const grant=context.parameters.authorization;
    if(await this.journal.get(`revoked:${grant}`))return failure('application binding was revoked; no action dispatched');
    const previous=await this.journal.get(key);
    if(previous)return previous.result??failure('application invocation was already dispatched; outcome is unknown',true);
    const credential=context.credential?.token;
    if(typeof credential!=='string' || credential.length<32 || credential.length>4096)return failure('application credential unavailable');
    const tool=admitted.parameters.tools.find(tool=>tool.name===operation);
    if(!tool)return failure('application operation is unavailable');
    const token=randomBytes(32).toString('hex'),verifier=createHash('sha256').update(token).digest('hex');
    await this.journal.set(key,{id:key,grant,tenant:context.tenant,session:context.session,verifier,deadline,phase:'dispatched'});
    const committed=new Promise(resolve=>{this.waiters.set(verifier,{key,grant,resolve,progress});});
    try {
      signal.throwIfAborted();
      const pending=this.outbound(admitted.parameters.endpoint,{token:credential,signal,body:{...request,tool,deadline_at_ms:deadline,completion:{url:`${this.callbackOrigin}/v1/tool-completions/${token}`}}}).then(response=>{
        if(response?.content && Array.isArray(response.content))return response;
        throw new Error('invalid application result');
      }).catch(()=>failure('application request ended without a committed result; remote outcome may be unknown',true));
      const response=await Promise.race([pending,committed]);
      const saved=await this.journal.get(key);
      if(!saved)return failure('application invocation was revoked; remote outcome may be unknown',true);
      if(saved.result)return saved.result;
      saved.result=response;saved.phase='returned';await this.journal.set(key,saved);return response;
    } finally {this.waiters.delete(verifier);}
  }
  async complete(token,input) {
    const verifier=createHash('sha256').update(token).digest('hex');
    const job=await this.journal.completion(verifier);
    if(!job || job.deadline<=this.now())throw new Error('completion is expired or unknown');
    if(input?.type==='progress') {const pending=this.waiters.get(verifier);if(!pending)throw new Error('invocation is no longer attached');pending.progress(input.value);return {delivered:true};}
    const response=input?.type==='finish'?input.result:undefined;
    if(!response?.content || !Array.isArray(response.content) || Buffer.byteLength(JSON.stringify(response))>2*1024*1024)throw new Error('invalid tool result');
    if(job.result && !isDeepStrictEqual(job.result,response))throw new Error('completion already committed');
    job.result=response;job.phase='finished';await this.journal.set(job.id,job);
    this.waiters.get(verifier)?.resolve(response);return {committed:true};
  }
  async maintain(control) {
    const pending=await control('pending');
    for(const grant of pending.grants)if(grant.provider==='application') {
      await this.journal.set(`revoked:${grant.authorization}`,{id:`revoked:${grant.authorization}`,phase:'revoked'});
      for(const pending of this.active.values())if(pending.grant===grant.authorization)pending.controller.abort();
      await this.journal.deleteGrant(grant.authorization);
      await control('cleaned',{authorization:grant.authorization,tenant:grant.tenant,session:grant.session});
    }
  }
}
