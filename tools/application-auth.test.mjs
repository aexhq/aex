import assert from "node:assert/strict";
import test from "node:test";
import { createHmac } from "node:crypto";
import { authenticate } from "../services/tools/common.mjs";
test("sealed application credentials authenticate the service and expose only request-local authority",()=>{
 const token="bridge-secret".repeat(3),credential="customer-secret".repeat(3),envelope={tenant:"a",session:"s",provider:"application",alias:"app",parameters:{authorization:"grant"},credential:{token:credential}};
 const bearer=value=>{const body=Buffer.from(JSON.stringify(value)).toString("base64url");return `Bearer ${body}.${createHmac("sha256",token).update(body).digest("base64url")}`;};
 assert.deepEqual(authenticate(bearer(envelope),token),envelope);assert.throws(()=>authenticate(bearer(envelope),"forged"));
 for(const header of [undefined,"Bearer invalid","Bearer application.invalid",bearer(envelope).replace(/.$/,"!")])assert.throws(()=>authenticate(header,token));
});
