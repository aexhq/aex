import { Pi, Aex, Model, ApiKey, Tool, Application, type AexSessionHandle, type Session } from "../../dist/index.js";
declare const aex:Aex;
declare const session:AexSessionHandle;
const reopened:Promise<AexSessionHandle>=aex.sessions.get("session");
const listed:Promise<{sessions:Session[]}>=aex.sessions.list();
const created:Promise<AexSessionHandle>=aex.sessions.create({harness: Pi(),model:Model("openai/gpt-4.1-mini",ApiKey("private")),tools:[Tool({name:"read",description:"Read",input_schema:{type:"object"},async run(){return "done";}})]});
const app=Application({endpoint:"https://application.example/tools",credential:"x".repeat(32),tools:[{name:"read",description:"Read",input_schema:{type:"object"}}]});
const binding:Promise<void>=session.rebind([]);
// @ts-expect-error Runtime package commands do not belong in a hosted session.
aex.sessions.create({harness:"pi",command:["customer-code"]});
void [reopened,listed,created,app,binding];
