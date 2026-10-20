import { request } from 'node:https';
import { lookup } from 'node:dns';
import { BlockList, isIP } from 'node:net';

const denied=new BlockList();
for(const [address,bits] of [['0.0.0.0',8],['10.0.0.0',8],['100.64.0.0',10],['127.0.0.0',8],['169.254.0.0',16],['172.16.0.0',12],['192.0.0.0',24],['192.0.2.0',24],['192.168.0.0',16],['198.18.0.0',15],['198.51.100.0',24],['203.0.113.0',24],['224.0.0.0',3]])denied.addSubnet(address,bits);
const globalV6=new BlockList();globalV6.addSubnet('2000::',3,'ipv6');
for(const [address,bits] of [['2001::',23],['2002::',16],['2001:db8::',32]])denied.addSubnet(address,bits,'ipv6');
export function publicAddress(address){return isIP(address)===4?!denied.check(address):isIP(address)===6 && globalV6.check(address,'ipv6') && !denied.check(address,'ipv6');}
export function publicJSON(url,{token,body,signal}) {
  const destination=new URL(url),host=destination.hostname.replace(/^\[|\]$/g,'');
  if(destination.protocol!=='https:' || destination.port && destination.port!=='443' || destination.username || destination.password || destination.search || destination.hash || isIP(host) && !publicAddress(host))throw new Error('application endpoint must be public HTTPS/443');
  return new Promise((resolve,reject)=>{
    const req=request(destination,{method:'POST',agent:false,signal,headers:{authorization:`Bearer ${token}`,'content-type':'application/json'},lookup:(host,options,callback)=>lookup(host,{all:true},(error,addresses)=>{
      if(error)return callback(error);if(!addresses.length || addresses.some(({address})=>!publicAddress(address)))return callback(new Error('application DNS resolves to a private address'));
      if(options.all)callback(null,addresses);else callback(null,addresses[0].address,addresses[0].family);
    })},async res=>{
      try {
        if(res.statusCode!==200 || !res.headers['content-type']?.startsWith('application/json')){res.destroy();throw new Error('invalid application response');}
        let size=0;const chunks=[];for await(const chunk of res){size+=chunk.length;if(size>2*1024*1024)throw new Error('application response exceeds limit');chunks.push(chunk);}
        resolve(JSON.parse(Buffer.concat(chunks)));
      } catch(error){res.destroy();reject(error);}
    });
    req.once('error',reject);req.end(JSON.stringify(body));
  });
}
