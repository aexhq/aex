import assert from "node:assert/strict";
import test from "node:test";
import { z } from "zod";
import { Aex, AexSessionHandle, Brain } from "../dist/index.js";

test("session lifecycle delegates close, rebind and deletion to its owning Brain client",async()=>{
  const calls=[];const client=new Brain("https://brain.example","token",async(url,init)=>{calls.push({url:String(url),init});return Response.json({});});
  const session=new AexSessionHandle(client,{id:"s",status:"idle"});await session.close();await session.delete();assert.deepEqual(calls.map(call=>[new URL(call.url).pathname,call.init.method]),[["/sessions/s/close","POST"],["/sessions/s","DELETE"]]);await client.dispose();
});

test("Aex close aborts a typed answer read and blocks later sends", async () => {
  const reading = Promise.withResolvers();
  let messages = 0;
  const client = new Aex({ apiKey: "key", fetch: async (url, init) => {
    url=String(url);
    if (url.endsWith("/events") && init.method === "POST") {
      messages++;
      return Response.json({ id: "input", cursor: "accepted", type:"user.message", time:"2026-10-20T00:00:00Z", data:{} });
    }
    if (url.includes("/events?")) {
      reading.resolve();
      return new Promise((_, reject) => init.signal.addEventListener("abort", () => reject(init.signal.reason), { once: true }));
    }
    return Response.json({ id: "s", status: "idle" });
  } });
  const session = await client.sessions.get("s");
  const pending = assert.rejects(session.send("Extract", { output: { type: z.string() } }), { name: "AbortError" });
  await reading.promise;
  await client.close();
  await pending;
  await assert.rejects(session.send("Later"), { name: "AbortError" });
  assert.equal(messages, 1);
});

test("typed sends exclude submit and release ownership after validation", async () => {
  const validating = Promise.withResolvers();
  const release = Promise.withResolvers();
  const client = new Aex({ apiKey: "key", fetch: async (url, init) => {
    url=String(url);
    if(url.endsWith("/events") && init.method==="POST")return Response.json({id:"input",cursor:"accepted",type:"user.message",time:"2026-10-20T00:00:00Z",data:{}});
    if(url.includes("/events?"))return Response.json({events:[{id:"answer",cursor:"answer",input_id:"input",time:"2026-10-20T00:00:00Z",type:"assistant.message",data:{text:'"answer"'}},{id:"done",cursor:"done",input_id:"input",time:"2026-10-20T00:00:00Z",type:"session.status",data:{status:"idle"}}],next_cursor:"done"});
    return Response.json({ id: "s", status: "idle" });
  } });
  const session = await client.sessions.get("s");
  const pending = session.send("Extract", { output: { type: z.string().refine(async () => { validating.resolve(); await release.promise; return true; }) } });
  await validating.promise;
  await assert.rejects(session.submit("Overlap"), /exclusive sends/);
  release.resolve();
  assert.equal(await pending, "answer");
  assert.equal((await session.submit("Later")).id,"input");

  await client.close();
});
