import { Aex,Tool } from "@aexhq/sdk";
let aex,attached,access,baseUrl,composition;
const tools=[Tool({name:"lookup",description:"Read the current selection",input_schema:{type:"object",properties:{id:{type:"string"}},required:["id"],additionalProperties:false},run:({id})=>`${document.querySelector("#selection").textContent}-${id}`})];
window.prepareBrowserJourney=input=>{({baseUrl,access}=input);aex=new Aex({baseUrl,clientAccess:access});};
window.disconnectBrowserJourney=async()=>{composition??=await aex.clients.composition();await attached?.detach();aex=new Aex({baseUrl,clientAccess:access});};
window.browserComposition=()=>composition;
document.querySelector("#attach").addEventListener("click",()=>{
  const connecting=composition?.session?aex.attachTab({session:composition.session,tools}):aex.attachTab({create:{...access.session,runtime_tools:tools}});
  connecting.then(async result=>{attached=result;if(composition?.session){window.browserReattached=true;return;}await result.session.send('CALL runtime_lookup {"id":"42"}');window.browserResult={id:result.session.id,transcript:await result.session.transcript()};}).catch(error=>{window.browserFailure=error.message;});
});
