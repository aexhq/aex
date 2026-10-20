import { result, failure } from './common.mjs';

export class Sandboxes {
  constructor({journal,provider,control,now=Date.now}){Object.assign(this,{journal,provider,control,now});this.active=new Set();}
  async invoke(operation,context,request,admitted) {
    const id=context.parameters.authorization;
    if(this.active.has(id))return failure('sandbox operation already in progress; no action dispatched');
    this.active.add(id);
    try {
      let job=this.journal.get(id);
      if(operation==='create') {
        if(job)return job.phase==='ready'?result({sandbox:id}):failure('sandbox creation outcome is unresolved; no action replayed',true);
        this.provider.validate?.(admitted.profile);
        if(admitted.expires_ms-this.now()<1000)return failure('sandbox binding has expired; no allocation dispatched');
        job={id,tenant:context.tenant,session:context.session,phase:'allocating',started:null,stopped:null,resources:[],expires:admitted.expires_ms};
        this.journal.set(id,job);
        // Record the dispatch boundary before the provider call. An uncertain
        // allocation is reconciled by its provider tag, never repeated.
        try {
          const resource=await this.provider.create(id,admitted.profile,Math.max(1,job.expires-this.now()));
          job.resources=[resource];job.started=this.now();job.phase='ready';this.journal.set(id,job);
        } catch {return failure('sandbox allocation outcome is unknown; resource reconciliation is required',true);}
        await this.report(job,false);return result({sandbox:id});
      }
      if(!job || job.tenant!==context.tenant || job.session!==context.session || request.arguments.sandbox!==id)return failure('sandbox is not owned by this binding');
      if(operation==='terminate'){await this.stop(job);return result({state:'stopped'});}
      if(job.phase!=='ready')return failure('sandbox is unavailable; no action dispatched');
      if(operation==='inspect')return result(await this.provider.inspect(job.resources[0]));
      if(operation==='exec') {
        if(typeof request.arguments.command!=='string' || request.arguments.command.length>16384)return failure('invalid sandbox command');
        // Calls are accepted once even if the provider response is lost.
        const key=`call:${context.session}:${request.call_id}`;
        if(this.journal.get(key))return failure('command was already dispatched; its outcome may be unknown',true);
        this.journal.set(key,{id:key,grant:id,phase:'dispatched'});
        try {const response=await this.provider.exec(job.resources[0],request.arguments.command);this.journal.set(key,{id:key,grant:id,phase:'complete'});return result(response);}
        catch {return failure('sandbox command failed; its outcome may be unknown',true);}
      }
      return failure('unsupported sandbox operation');
    } finally {this.active.delete(id);}
  }
  async report(job,terminal) {
    if(job.started===null)throw new Error('sandbox allocation usage remains unknown');
    const units=Math.max(0,Math.min(job.stopped??this.now(),job.expires)-job.started);
    await this.control('usage',{authorization:job.id,tenant:job.tenant,session:job.session,units_ms:units,terminal,receipt:`sandbox:${job.id}:${units}:${terminal}`});
    if(terminal){job.phase='settled';this.journal.set(job.id,job);}
  }
  async stop(job) {
    if(job.phase==='settled')return;
    if(job.phase==='unused') {
      await this.control('usage',{authorization:job.id,tenant:job.tenant,session:job.session,units_ms:0,terminal:true,receipt:`sandbox-unused:${job.id}`});job.phase='settled';this.journal.set(job.id,job);return;
    }
    if(job.started===null) {
      job.resources=await this.provider.find(job.id);
      this.journal.set(job.id,job);
      // Finding no live resource does not establish whether an allocation ran
      // and stopped. Keep its reservation and explicit unknown outcome.
      if(!job.resources.length)throw new Error('allocation requires provider evidence');
      for(const resource of job.resources)await this.provider.terminate(resource);
      job.phase='usage_unknown';this.journal.set(job.id,job);throw new Error('resource removed; allocation start time requires operator evidence');
    }
    for(const resource of job.resources)await this.provider.terminate(resource);
    job.stopped??=this.now();job.phase='stopped';this.journal.set(job.id,job);await this.report(job,true);
  }
  async maintain() {
    const pending=await this.control('pending');
    for(const grant of pending.grants) {
      if(grant.provider && grant.provider!=='sandbox')continue;
      if(this.active.has(grant.authorization))continue;
      this.active.add(grant.authorization);
      try {
        let job=this.journal.get(grant.authorization);
        if(!job) {job={id:grant.authorization,tenant:grant.tenant,session:grant.session,phase:'unused',started:null};this.journal.set(job.id,job);}
        try {await this.stop(job);}catch {console.error(JSON.stringify({event:'sandbox_unresolved',grant:grant.authorization}));}
      } finally {this.active.delete(grant.authorization);}
    }
    for(const job of this.journal.values())if(job.phase==='ready' && !this.active.has(job.id)) {
      if(job.expires<=this.now())await this.stop(job);else await this.report(job,false);
    }
  }
}
