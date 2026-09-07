import { performance } from "node:perf_hooks";

export async function measure(url, token, count = 100) {
  const samples=[];
  for(let i=0;i<count+5;i++) {
    const start=performance.now();
    const response=await fetch(url,{headers:{authorization:`Bearer ${token}`}});
    if(!response.ok) throw new Error(`benchmark request failed: ${response.status}`);
    await response.arrayBuffer();
    if(i>=5) samples.push(performance.now()-start);
  }
  samples.sort((a,b)=>a-b);
  return {requests:count,p50_ms:samples[Math.floor(count*.5)],p95_ms:samples[Math.floor(count*.95)],max_ms:samples.at(-1)};
}
