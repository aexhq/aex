import { ModalClient } from 'modal';

async function bounded(stream) {
  let text='',size=0;for await(const chunk of stream){size+=Buffer.byteLength(chunk);if(size>65536)throw new Error('sandbox output limit exceeded');text+=chunk;}return text;
}
export function modalProvider(env=process.env) {
  if(!env.MODAL_TOKEN_ID || !env.MODAL_TOKEN_SECRET || !env.MODAL_APP)throw new Error('Modal credentials and application required');
  const client=new ModalClient({tokenId:env.MODAL_TOKEN_ID,tokenSecret:env.MODAL_TOKEN_SECRET,timeoutMs:60000});
  const app=()=>client.apps.fromName(env.MODAL_APP,{createIfMissing:true});
  const validate=profile=>{
    if(!profile || profile.cpu!==1 || profile.memoryMiB!==1024 || typeof profile.image!=='string' || !profile.image || Object.keys(profile).some(key=>!['image','cpu','memoryMiB','workdir','regions','outboundDomains','blockNetwork'].includes(key)))throw new Error('invalid installed 1 CPU / 1024 MiB sandbox profile');
    if(profile.regions!==undefined && (!Array.isArray(profile.regions) || !profile.regions.length || profile.regions.some(region=>typeof region!=='string' || !region)))throw new Error('invalid sandbox regions');
    if(profile.outboundDomains!==undefined && (!Array.isArray(profile.outboundDomains) || profile.outboundDomains.some(domain=>typeof domain!=='string' || !domain)))throw new Error('invalid sandbox network policy');
    if(profile.blockNetwork===true && profile.outboundDomains?.length)throw new Error('conflicting sandbox network policy');
  };
  return {
    validate,
    async create(grant,profile,lifetime){
      validate(profile);
      const image=profile.image.startsWith('im-')?await client.images.fromId(profile.image):client.images.fromRegistry(profile.image);
      const network=profile.outboundDomains?.length?{outboundCidrAllowlist:[],outboundDomainAllowlist:profile.outboundDomains}:{blockNetwork:profile.blockNetwork??true};
      const sandbox=await client.sandboxes.create(await app(),image,{cpu:1,memoryMiB:1024,memoryLimitMiB:1024,timeoutMs:Math.floor(lifetime/1000)*1000,...network,...(profile.workdir?{workdir:profile.workdir}:{}),...(profile.regions?{regions:profile.regions}:{}),tags:{service:'aex',grant}});
      return sandbox.sandboxId;
    },
    async find(grant){const matches=[];for await(const sandbox of client.sandboxes.list({appId:(await app()).appId,tags:{service:'aex',grant}}))matches.push(sandbox.sandboxId);return matches;},
    async exec(id,command){
      const sandbox=await client.sandboxes.fromId(id);if(await sandbox.poll()!==null)throw new Error('sandbox is stopped');
      const process=await sandbox.exec(['/bin/sh','-lc',command],{timeoutMs:30000});
      const [stdout,stderr,exit_code]=await Promise.all([bounded(process.stdout),bounded(process.stderr),process.wait()]);return {stdout,stderr,exit_code};
    },
    async inspect(id){const exit_code=await(await client.sandboxes.fromId(id)).poll();return {state:exit_code===null?'running':'stopped',exit_code};},
    async terminate(id){await(await client.sandboxes.fromId(id)).terminate({wait:true});},
    close(){client.close();},
  };
}
